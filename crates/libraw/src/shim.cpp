#include "shim.h"

#include <cstdio>
#include <cstring>
#include <new>

#include "libraw/libraw.h"

// No C++ exception may unwind into Rust: every entry point catches everything.
#define PL_GUARD(expr)                     \
    try {                                  \
        expr;                              \
    } catch (...) {                        \
        return LIBRAW_UNSPECIFIED_ERROR;   \
    }

static void copy_str(char *dst, size_t cap, const char *src) {
    std::strncpy(dst, src, cap - 1);
    dst[cap - 1] = '\0';
}

static void fill_image(libraw_processed_image_t *img, pl_image *out) {
    out->kind = static_cast<int32_t>(img->type);
    out->width = img->width;
    out->height = img->height;
    out->colors = img->colors;
    out->bits = img->bits;
    out->data = img->data;
    out->len = img->data_size;
    out->handle = img;
}

extern "C" {

void *pl_new(void) {
    try {
        return new (std::nothrow) LibRaw(0);
    } catch (...) {
        return nullptr;
    }
}

void pl_free(void *raw) {
    try {
        delete static_cast<LibRaw *>(raw);
    } catch (...) {
    }
}

int pl_open_buffer(void *raw, const void *buffer, size_t size) {
    PL_GUARD(return static_cast<LibRaw *>(raw)->open_buffer(buffer, size));
}

void pl_get_info(void *raw, pl_info *out) {
    std::memset(out, 0, sizeof(*out));
    const libraw_data_t &d = static_cast<LibRaw *>(raw)->imgdata;
    copy_str(out->make, sizeof(out->make), d.idata.make);
    copy_str(out->model, sizeof(out->model), d.idata.model);
    copy_str(out->lens, sizeof(out->lens), d.lens.Lens);
    if (!out->lens[0]) copy_str(out->lens, sizeof(out->lens), d.lens.makernotes.Lens);
    if (!out->lens[0]) {
        // Nikon NEFs (and others) often only carry the lens' focal/aperture range;
        // describe it the way converters do, e.g. "24.0-70.0 mm f/2.8" or "60.0 mm f/2.8".
        const libraw_makernotes_lens_t &m = d.lens.makernotes;
        float lo = m.MinFocal > 0 ? m.MinFocal : d.lens.MinFocal;
        float hi = m.MaxFocal > 0 ? m.MaxFocal : d.lens.MaxFocal;
        float ap = m.MaxAp4MinFocal > 0 ? m.MaxAp4MinFocal : d.lens.MaxAp4MinFocal;
        float ap2 = m.MaxAp4MaxFocal > 0 ? m.MaxAp4MaxFocal : d.lens.MaxAp4MaxFocal;
        if (lo > 0 && hi > 0) {
            int n = (lo == hi) ? std::snprintf(out->lens, sizeof(out->lens), "%.1f mm", lo)
                               : std::snprintf(out->lens, sizeof(out->lens), "%.1f-%.1f mm", lo, hi);
            if (n > 0 && ap > 0 && static_cast<size_t>(n) < sizeof(out->lens)) {
                if (ap2 > 0 && ap2 != ap)
                    std::snprintf(out->lens + n, sizeof(out->lens) - n, " f/%.1f-%.1f", ap, ap2);
                else
                    std::snprintf(out->lens + n, sizeof(out->lens) - n, " f/%.1f", ap);
            }
        }
    }
    out->iso = d.other.iso_speed;
    out->shutter = d.other.shutter;
    out->aperture = d.other.aperture;
    out->focal_len = d.other.focal_len;
    out->timestamp = static_cast<int64_t>(d.other.timestamp);
    out->width = d.sizes.width;
    out->height = d.sizes.height;
    out->flip = d.sizes.flip;
    out->thumb_format = static_cast<int32_t>(d.thumbnail.tformat);
    out->thumb_width = d.thumbnail.twidth;
    out->thumb_height = d.thumbnail.theight;

    const libraw_gps_info_t &g = d.other.parsed_gps;
    auto degrees = [](const float v[3]) {
        return static_cast<double>(v[0]) + v[1] / 60.0 + v[2] / 3600.0;
    };
    double lat = degrees(g.latitude), lon = degrees(g.longitude);
    bool refs = (g.latref == 'N' || g.latref == 'S') && (g.longref == 'E' || g.longref == 'W');
    if (g.gpsparsed && refs && (lat != 0.0 || lon != 0.0) && lat <= 90.0 && lon <= 180.0) {
        out->has_gps = 1;
        out->gps_lat = g.latref == 'S' ? -lat : lat;
        out->gps_lon = g.longref == 'W' ? -lon : lon;
        if (g.altitude != 0.0f) {
            out->has_alt = 1;
            out->gps_alt = g.altref == 1 ? -g.altitude : g.altitude;
        }
    }
}

int pl_thumbnail(void *raw, pl_image *out) {
    std::memset(out, 0, sizeof(*out));
    PL_GUARD({
        LibRaw *lr = static_cast<LibRaw *>(raw);
        int rc = lr->unpack_thumb();
        if (rc != LIBRAW_SUCCESS) return rc;
        int err = LIBRAW_SUCCESS;
        libraw_processed_image_t *img = lr->dcraw_make_mem_thumb(&err);
        if (!img) return err != LIBRAW_SUCCESS ? err : LIBRAW_UNSPECIFIED_ERROR;
        fill_image(img, out);
        return LIBRAW_SUCCESS;
    });
}

enum RenderMode { SRGB8, LINEAR_PROPHOTO16, CAMERA16 };

static int render(void *raw, int half_size, RenderMode mode, pl_image *out) {
    std::memset(out, 0, sizeof(*out));
    PL_GUARD({
        LibRaw *lr = static_cast<LibRaw *>(raw);
        libraw_output_params_t &p = lr->imgdata.params;
        p.use_camera_wb = 1;  // as-shot white balance
        p.user_mul[0] = p.user_mul[1] = p.user_mul[2] = p.user_mul[3] = 0.0f;
        p.no_auto_bright = 0;
        p.gamm[0] = 0.45;  // LibRaw's defaults (BT.709 curve)
        p.gamm[1] = 4.5;
        switch (mode) {
        case SRGB8:
            p.output_bps = 8;
            p.output_color = 1;  // sRGB
            break;
        case LINEAR_PROPHOTO16:
            p.output_bps = 16;
            p.output_color = 4;  // ProPhoto
            p.gamm[0] = 1.0;     // linear
            p.gamm[1] = 1.0;
            break;
        case CAMERA16:
            p.output_bps = 16;
            p.output_color = 0;  // raw camera colour
            p.gamm[0] = 1.0;
            p.gamm[1] = 1.0;
            p.use_camera_wb = 0;
            p.user_mul[0] = p.user_mul[1] = p.user_mul[2] = p.user_mul[3] = 1.0f;
            p.no_auto_bright = 1;
            break;
        }
        p.half_size = half_size ? 1 : 0;
        // unpack() may run once per open file; dcraw_process() can then run repeatedly.
        int rc = LIBRAW_SUCCESS;
        if (!(lr->imgdata.progress_flags & LIBRAW_PROGRESS_LOAD_RAW)) {
            rc = lr->unpack();
            if (rc != LIBRAW_SUCCESS) return rc;
        }
        rc = lr->dcraw_process();
        if (rc != LIBRAW_SUCCESS) return rc;
        int err = LIBRAW_SUCCESS;
        libraw_processed_image_t *img = lr->dcraw_make_mem_image(&err);
        if (!img) return err != LIBRAW_SUCCESS ? err : LIBRAW_UNSPECIFIED_ERROR;
        fill_image(img, out);
        return LIBRAW_SUCCESS;
    });
}

int pl_render(void *raw, int half_size, pl_image *out) { return render(raw, half_size, SRGB8, out); }

int pl_render16_linear_prophoto(void *raw, int half_size, pl_image *out) {
    return render(raw, half_size, LINEAR_PROPHOTO16, out);
}

int pl_render16_camera(void *raw, int half_size, pl_image *out) {
    return render(raw, half_size, CAMERA16, out);
}

void pl_get_color(void *raw, pl_color *out) {
    std::memset(out, 0, sizeof(*out));
    const libraw_data_t &d = static_cast<LibRaw *>(raw)->imgdata;
    out->colors = d.idata.colors;
    for (int c = 0; c < 4; c++) {
        out->cam_mul[c] = d.color.cam_mul[c];
        out->pre_mul[c] = d.color.pre_mul[c];
        for (int i = 0; i < 3; i++) {
            out->rgb_cam[i][c] = d.color.rgb_cam[i][c];
            out->cam_xyz[c][i] = d.color.cam_xyz[c][i];
        }
    }
}

void pl_image_free(pl_image *img) {
    if (img && img->handle) {
        LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t *>(img->handle));
        img->handle = nullptr;
        img->data = nullptr;
        img->len = 0;
    }
}

const char *pl_strerror(int code) { return LibRaw::strerror(code); }

const char *pl_version(void) { return LibRaw::version(); }

}  // extern "C"

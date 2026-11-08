#include "shim.h"

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

int pl_render(void *raw, int half_size, pl_image *out) {
    std::memset(out, 0, sizeof(*out));
    PL_GUARD({
        LibRaw *lr = static_cast<LibRaw *>(raw);
        libraw_output_params_t &p = lr->imgdata.params;
        p.output_bps = 8;
        p.output_color = 1;   // sRGB
        p.use_camera_wb = 1;  // as-shot white balance
        p.half_size = half_size ? 1 : 0;
        int rc = lr->unpack();
        if (rc != LIBRAW_SUCCESS) return rc;
        rc = lr->dcraw_process();
        if (rc != LIBRAW_SUCCESS) return rc;
        int err = LIBRAW_SUCCESS;
        libraw_processed_image_t *img = lr->dcraw_make_mem_image(&err);
        if (!img) return err != LIBRAW_SUCCESS ? err : LIBRAW_UNSPECIFIED_ERROR;
        fill_image(img, out);
        return LIBRAW_SUCCESS;
    });
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

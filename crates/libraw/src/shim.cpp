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

// A file read on demand through a callback, a window at a time (so LibRaw's many
// byte-wise reads stay here and only window refills cross to the caller). Windows are
// aligned, so the caller can keep and reuse the same blocks.
class PlStream : public LibRaw_abstract_datastream {
  public:
    static const size_t WINDOW = 64 * 1024;

    PlStream(pl_read_at read_at, void *ctx, uint64_t size)
        : read_at_(read_at), ctx_(ctx), size_(size), pos_(0), win_start_(0), win_len_(0) {
        win_ = new unsigned char[WINDOW];
    }
    ~PlStream() override { delete[] win_; }

    int valid() override { return read_at_ ? 1 : 0; }
    INT64 size() override { return static_cast<INT64>(size_); }
    INT64 tell() override { return static_cast<INT64>(pos_); }
    int eof() override { return pos_ >= size_; }

    int get_char() override {
        if (!in_window() && !fill())
            return -1;
        return win_[pos_++ - win_start_];
    }

    int read(void *ptr, size_t sz, size_t nmemb) override {
        if (pos_ >= size_)
            return 0;
        uint64_t left = size_ - pos_;
        size_t want = sz * nmemb;
        size_t to_read = want > left ? static_cast<size_t>(left) : want;
        if (to_read < 1)
            return 0;
        unsigned char *out = static_cast<unsigned char *>(ptr);
        size_t done = 0;
        while (done < to_read) {
            if (in_window()) {
                size_t off = static_cast<size_t>(pos_ - win_start_);
                size_t n = win_len_ - off;
                if (n > to_read - done)
                    n = to_read - done;
                std::memcpy(out + done, win_ + off, n);
                done += n;
                pos_ += n;
            } else if (to_read - done >= WINDOW) {
                // A big read: straight into the caller's memory.
                size_t n = read_at_(ctx_, pos_, out + done, to_read - done);
                done += n;
                pos_ += n;
                if (n == 0)
                    break;
            } else if (!fill()) {
                break;
            }
        }
        return static_cast<int>((done + sz - 1) / (sz > 0 ? sz : 1));
    }

    int seek(INT64 o, int whence) override {
        INT64 base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? static_cast<INT64>(pos_) : whence == SEEK_END ? static_cast<INT64>(size_) : -1;
        if (base < 0 && whence != SEEK_SET)
            return 0;
        INT64 to = base + o;
        if (to < 0)
            to = 0;
        if (static_cast<uint64_t>(to) > size_)
            to = static_cast<INT64>(size_);
        pos_ = static_cast<uint64_t>(to);
        return 0;
    }

    char *gets(char *s, int sz) override {
        if (sz < 1 || pos_ >= size_)
            return nullptr;
        int n = 0;
        while (n < sz - 1) {
            int c = get_char();
            if (c < 0)
                break;
            s[n++] = static_cast<char>(c);
            if (c == '\n')
                break;
        }
        s[n] = 0;
        return s;
    }

    int scanf_one(const char *fmt, void *val) override {
        // As LibRaw's own streams: parse from the next 24 bytes, then skip the token.
        if (size_ < 24 || pos_ > size_ - 24)
            return 0;
        char text[25];
        uint64_t start = pos_;
        for (int i = 0; i < 24; i++) {
            int c = get_char();
            text[i] = c < 0 ? 0 : static_cast<char>(c);
        }
        text[24] = 0;
        pos_ = start;
        int res = std::sscanf(text, fmt, val);
        if (res > 0) {
            int xcnt = 0;
            while (pos_ < size_ - 1) {
                pos_++;
                xcnt++;
                uint64_t keep = pos_;
                int c = get_char();
                pos_ = keep;
                if (c <= 0 || c == ' ' || c == '\t' || c == '\n' || xcnt > 24)
                    break;
            }
        }
        return res;
    }

  private:
    bool in_window() const { return pos_ >= win_start_ && pos_ < win_start_ + win_len_; }

    bool fill() {
        if (pos_ >= size_)
            return false;
        uint64_t start = pos_ - (pos_ % WINDOW);
        uint64_t left = size_ - start;
        size_t want = left < WINDOW ? static_cast<size_t>(left) : WINDOW;
        win_start_ = start;
        win_len_ = read_at_(ctx_, start, win_, want);
        return in_window();
    }

    pl_read_at read_at_;
    void *ctx_;
    uint64_t size_;
    uint64_t pos_;
    unsigned char *win_;
    uint64_t win_start_;
    size_t win_len_;
};

extern "C" {

int pl_open_stream(void *raw, pl_read_at read_at, void *ctx, uint64_t size, void **stream_out) {
    *stream_out = nullptr;
    try {
        PlStream *stream = new PlStream(read_at, ctx, size);
        *stream_out = stream;
        return static_cast<LibRaw *>(raw)->open_datastream(stream);
    } catch (...) {
        return LIBRAW_UNSPECIFIED_ERROR;
    }
}

void pl_stream_free(void *stream) {
    try {
        delete static_cast<PlStream *>(stream);
    } catch (...) {
    }
}

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
                    n += std::snprintf(out->lens + n, sizeof(out->lens) - n, " f/%.1f-%.1f", ap, ap2);
                else
                    n += std::snprintf(out->lens + n, sizeof(out->lens) - n, " f/%.1f", ap);
            }
            // Nikon's lens type from the maker notes ("G", "D", "E", "G VR"): tells lenses
            // of the same focal length and aperture apart (an AF-D from an AF-S G).
            if (n > 0 && m.LensFeatures_suf[0] && static_cast<size_t>(n) < sizeof(out->lens))
                std::snprintf(out->lens + n, sizeof(out->lens) - n, " %s", m.LensFeatures_suf);
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

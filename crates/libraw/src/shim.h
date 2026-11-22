// Flat, stable C interface over LibRaw used by the Rust side.
// Keeps Rust independent of LibRaw's large and version-dependent structs.
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct pl_info {
    char make[64];
    char model[64];
    char lens[128];
    float iso;
    float shutter;   // seconds
    float aperture;  // f-number
    float focal_len; // mm
    int64_t timestamp; // unix seconds, 0 if unknown
    int32_t width;     // output image size before rotation
    int32_t height;
    int32_t flip;      // LibRaw flip code: 0, 3 (180), 5 (90 CCW), 6 (90 CW)
    int32_t thumb_format;
    int32_t thumb_width;
    int32_t thumb_height;
    int32_t has_gps;   // 1 if latitude/longitude are set
    int32_t has_alt;   // 1 if altitude is set
    double gps_lat;    // decimal degrees, south negative
    double gps_lon;    // decimal degrees, west negative
    double gps_alt;    // metres, below sea level negative
} pl_info;

typedef struct pl_image {
    int32_t kind;  // 1 = JPEG, 2 = bitmap, 3 = JPEG XL
    int32_t width;
    int32_t height;
    int32_t colors;
    int32_t bits;
    const uint8_t *data;
    size_t len;
    void *handle;  // owned libraw_processed_image_t*, release with pl_image_free
} pl_image;

// Colour data for converting camera RGB ourselves (see pl_render16_camera).
typedef struct pl_color {
    int32_t colors;       // colour channels after demosaicing (3 for RGB sensors, 1 for monochrome)
    float cam_mul[4];     // as-shot white balance multipliers (all zero if unknown)
    float pre_mul[4];     // daylight multipliers (overwritten by a render: read right after opening)
    float rgb_cam[3][4];  // white-balanced camera RGB -> linear sRGB (D65)
    float cam_xyz[4][3];  // XYZ -> camera RGB (all zero if the camera has no matrix)
} pl_color;

void *pl_new(void);
void pl_free(void *raw);
// The buffer must stay alive and unchanged until pl_free.
int pl_open_buffer(void *raw, const void *buffer, size_t size);
// Read `len` bytes at `offset` into `buf`; returns how many were read (less at the end of
// the file or on an error). Must not unwind.
typedef size_t (*pl_read_at)(void *ctx, uint64_t offset, uint8_t *buf, size_t len);
// Open a file of `size` bytes that is read through `read_at` as LibRaw asks for it (no
// memory map, no whole-file read). `*stream_out` gets a handle to release with
// pl_stream_free AFTER pl_free; `ctx` must stay valid until then.
int pl_open_stream(void *raw, pl_read_at read_at, void *ctx, uint64_t size, void **stream_out);
void pl_stream_free(void *stream);
void pl_get_info(void *raw, pl_info *out);
int pl_thumbnail(void *raw, pl_image *out);
int pl_render(void *raw, int half_size, pl_image *out);
// 16 bits per channel, linear, ProPhoto primaries (LibRaw's "ProPhoto D65"), for
// exports that keep the raw data's full precision and gamut.
int pl_render16_linear_prophoto(void *raw, int half_size, pl_image *out);
// 16 bits per channel, linear, raw camera RGB with unit white balance multipliers (no
// white balance, no colour matrix): the caller applies both. Clipped sensor values are 65535.
int pl_render16_camera(void *raw, int half_size, pl_image *out);
void pl_get_color(void *raw, pl_color *out);
void pl_image_free(pl_image *img);
const char *pl_strerror(int code);
const char *pl_version(void);

#ifdef __cplusplus
}
#endif

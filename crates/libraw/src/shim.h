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

void *pl_new(void);
void pl_free(void *raw);
// The buffer must stay alive and unchanged until pl_free.
int pl_open_buffer(void *raw, const void *buffer, size_t size);
void pl_get_info(void *raw, pl_info *out);
int pl_thumbnail(void *raw, pl_image *out);
int pl_render(void *raw, int half_size, pl_image *out);
void pl_image_free(pl_image *img);
const char *pl_strerror(int code);
const char *pl_version(void);

#ifdef __cplusplus
}
#endif

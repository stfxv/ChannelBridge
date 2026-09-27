// SPDX-License-Identifier: GPL-3.0-only
#pragma once
#include <math.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    float *data;
    uint32_t channels;
    uint32_t frames;
} CBBuffer;

// A channel is a strided view; works with interleaved and planar HAL streams.
typedef struct { float *data; uint32_t stride, frames; } CBChannel;
static inline CBChannel cb_channel(const CBBuffer *buffers, uint32_t count, uint32_t channel) {
    for (uint32_t i = 0; i < count; i++) {
        if (channel < buffers[i].channels) {
            return (CBChannel){buffers[i].data ? buffers[i].data + channel : NULL,
                               buffers[i].channels, buffers[i].frames};
        }
        channel -= buffers[i].channels;
    }
    return (CBChannel){0};
}
static inline float cb_limit(float v) {
    if (!isfinite(v)) return 0;
    return fmaxf(-1, fminf(1, v));
}
// mono duplicates one input; stereo preserves L/R; mix averages two inputs.
// No allocations, locks, logging, Objective-C, or Go calls on the audio thread.
static inline int cb_mix(const CBBuffer *input, uint32_t inputs,
                         CBBuffer *output, uint32_t outputs,
                         uint32_t left, uint32_t right, int mode,
                         float gain, int muted, float *peakL, float *peakR) {
    CBChannel l = cb_channel(input, inputs, left);
    CBChannel r = cb_channel(input, inputs, mode == 0 ? left : right);
    CBChannel outL = cb_channel(output, outputs, 0);
    CBChannel outR = cb_channel(output, outputs, 1);
    *peakL = *peakR = 0;
    if (!outL.data || !outR.data) return 0;
    uint32_t frames = outL.frames < outR.frames ? outL.frames : outR.frames;
    if (muted) return 1; // Caller has already cleared all output buffers.
    if (!l.data || !r.data || l.frames < frames || r.frames < frames) return 0;
    for (uint32_t f = 0; f < frames; f++) {
        float a = l.data[f * l.stride], b = r.data[f * r.stride];
        if (mode == 2) a = b = (a + b) * 0.5f;
        a = cb_limit(a * gain); b = cb_limit(b * gain);
        outL.data[f * outL.stride] = a;
        outR.data[f * outR.stride] = b;
        *peakL = fmaxf(*peakL, fabsf(a)); *peakR = fmaxf(*peakR, fabsf(b));
    }
    return 1;
}

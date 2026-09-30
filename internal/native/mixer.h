// SPDX-License-Identifier: GPL-3.0-only
#pragma once
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdatomic.h>
#include <string.h>

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
static inline float cb_sample(float v) { return isfinite(v) ? v : 0; }
static inline float cb_limit(float v) { return fmaxf(-1, fminf(1, cb_sample(v))); }

// Positive IEEE float bit patterns sort as uint32. Lock-free peak storage retains
// sub-UI-interval transients and avoids the old 0.0001 amplitude quantization.
static inline void cb_hold_peak(_Atomic uint32_t *meter, float peak) {
    if (!isfinite(peak) || peak < 0) return;
    uint32_t bits; memcpy(&bits, &peak, sizeof(bits));
    uint32_t old = atomic_load_explicit(meter, memory_order_relaxed);
    while (bits > old && !atomic_compare_exchange_weak_explicit(meter, &old, bits,
            memory_order_relaxed, memory_order_relaxed)) {}
}
static inline float cb_take_peak(_Atomic uint32_t *meter) {
    uint32_t bits = atomic_exchange_explicit(meter, 0, memory_order_relaxed);
    float peak; memcpy(&peak, &bits, sizeof(peak)); return peak;
}

// Overview is opt-in. Walk buffers once and retain each physical channel's peak;
// aggregate virtual inputs are excluded by offset. No work for hidden overview.
static inline void cb_all_peaks(const CBBuffer *input, uint32_t inputs,
                                uint32_t offset, uint32_t count, _Atomic uint32_t *meters) {
    uint32_t base = 0;
    for (uint32_t i = 0; i < inputs; i++) {
        CBBuffer b = input[i];
        if (b.data) for (uint32_t ch = 0; ch < b.channels; ch++) {
            uint32_t absolute = base + ch;
            if (absolute < offset || absolute - offset >= count) continue;
            float peak = 0;
            for (uint32_t f = 0; f < b.frames; f++)
                peak = fmaxf(peak, fabsf(cb_sample(b.data[f * b.channels + ch])));
            cb_hold_peak(&meters[absolute - offset], peak);
        }
        base += b.channels;
    }
}

// Input-only metering has no output buffers and never routes audio.
static inline int cb_input_peaks(const CBBuffer *input, uint32_t inputs,
                                 uint32_t left, uint32_t right, float *peakL, float *peakR) {
    CBChannel selected[] = {cb_channel(input, inputs, left), cb_channel(input, inputs, right)};
    *peakL = *peakR = 0;
    if (!selected[0].data || !selected[1].data || !selected[0].frames || !selected[1].frames) return 0;
    float *peaks[] = {peakL, peakR};
    for (int ch = 0; ch < 2; ch++)
        for (uint32_t f = 0; f < selected[ch].frames; f++)
            *peaks[ch] = fmaxf(*peaks[ch], fabsf(cb_sample(selected[ch].data[f * selected[ch].stride])));
    return 1;
}

// Peaks describe each selected hardware input BEFORE mix, mute, and clipping.
// mono duplicates one input; stereo preserves L/R; mix averages two inputs.
// No allocations, locks, logging, Objective-C, or Go calls on the audio thread.
static inline int cb_mix(const CBBuffer *input, uint32_t inputs,
                         CBBuffer *output, uint32_t outputs,
                         uint32_t left, uint32_t right, int mode,
                         int muted, float *peakL, float *peakR) {
    CBChannel l = cb_channel(input, inputs, left);
    CBChannel r = cb_channel(input, inputs, mode == 0 ? left : right);
    CBChannel outL = cb_channel(output, outputs, 0);
    CBChannel outR = cb_channel(output, outputs, 1);
    *peakL = *peakR = 0;
    if (!outL.data || !outR.data || !l.data || !r.data) return 0;
    uint32_t frames = outL.frames < outR.frames ? outL.frames : outR.frames;
    if (l.frames < frames || r.frames < frames) return 0;
    for (uint32_t f = 0; f < frames; f++) {
        float a = cb_sample(l.data[f * l.stride]), b = cb_sample(r.data[f * r.stride]);
        *peakL = fmaxf(*peakL, fabsf(a)); *peakR = fmaxf(*peakR, fabsf(b));
        if (muted) continue; // Caller has already cleared every output buffer.
        if (mode == 2) a = b = a * 0.5f + b * 0.5f;
        outL.data[f * outL.stride] = cb_limit(a);
        outR.data[f * outR.stride] = cb_limit(b);
    }
    return 1;
}

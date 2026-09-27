// SPDX-License-Identifier: GPL-3.0-only
#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#include <stdatomic.h>
#include "bridge.h"
#include "mixer.h"

#define SEND_UID CFSTR("ChannelBridge_UID")
#define MAX_BUFFERS 512

typedef struct {
    AudioDeviceID aggregate, source, sink;
    AudioDeviceIOProcID proc;
    uint32_t left, right;
    int mode, muted;
    float gain;
    Float64 sourceRate;
    uint32_t inputChannels;
    _Atomic int invalidated;
    int listenerCount;
    _Atomic uint32_t peakL, peakR;
    _Atomic uint64_t callbacks, faults;
} Engine;
static Engine engine;

static AudioObjectPropertyAddress address(AudioObjectPropertySelector selector, AudioObjectPropertyScope scope) {
    return (AudioObjectPropertyAddress){selector, scope, kAudioObjectPropertyElementMain};
}
static OSStatus get(AudioObjectID object, AudioObjectPropertySelector selector,
                    AudioObjectPropertyScope scope, UInt32 *size, void *data) {
    AudioObjectPropertyAddress a = address(selector, scope);
    return AudioObjectGetPropertyData(object, &a, 0, NULL, size, data);
}
static NSString *stringProperty(AudioObjectID object, AudioObjectPropertySelector selector) {
    CFStringRef value = NULL; UInt32 size = sizeof(value);
    if (get(object, selector, kAudioObjectPropertyScopeGlobal, &size, &value)) return @"";
    return CFBridgingRelease(value);
}
static AudioDeviceID deviceForUID(CFStringRef uid) {
    AudioDeviceID result = kAudioObjectUnknown;
    UInt32 size = sizeof(result);
    AudioObjectPropertyAddress a = address(kAudioHardwarePropertyTranslateUIDToDevice, kAudioObjectPropertyScopeGlobal);
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &a, sizeof(uid), &uid, &size, &result))
        return kAudioObjectUnknown;
    return result;
}
static uint32_t channels(AudioDeviceID device, AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress a = address(kAudioDevicePropertyStreamConfiguration, scope);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &a, 0, NULL, &size) || !size) return 0;
    AudioBufferList *list = calloc(1, size);
    if (!list) return 0;
    uint32_t result = 0;
    if (!AudioObjectGetPropertyData(device, &a, 0, NULL, &size, list))
        for (UInt32 i = 0; i < list->mNumberBuffers; i++) result += list->mBuffers[i].mNumberChannels;
    free(list); return result;
}
static Float64 sampleRate(AudioDeviceID device) {
    Float64 rate = 0; UInt32 size = sizeof(rate);
    get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, &size, &rate);
    return rate;
}
char *cb_devices(void) {
    @autoreleasepool {
        AudioObjectPropertyAddress a = address(kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal);
        UInt32 size = 0;
        if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &a, 0, NULL, &size)) return strdup("[]");
        AudioDeviceID *ids = malloc(size);
        if (!ids) return strdup("[]");
        NSMutableArray *result = [NSMutableArray array];
        if (!AudioObjectGetPropertyData(kAudioObjectSystemObject, &a, 0, NULL, &size, ids)) {
            for (UInt32 i = 0; i < size / sizeof(AudioDeviceID); i++) {
                uint32_t count = channels(ids[i], kAudioObjectPropertyScopeInput);
                NSString *uid = stringProperty(ids[i], kAudioDevicePropertyDeviceUID);
                if (!count || [uid hasPrefix:@"ChannelBridge"] || [uid hasPrefix:@"local.channelbridge."]) continue;
                [result addObject:@{@"uid": uid, @"name": stringProperty(ids[i], kAudioObjectPropertyName),
                                    @"channels": @(count), @"rate": @(sampleRate(ids[i]))}];
            }
        }
        free(ids);
        NSData *json = [NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
        return strdup([[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding].UTF8String);
    }
}
int cb_device_available(const char *uid) {
    @autoreleasepool {
        NSString *value = [NSString stringWithUTF8String:uid];
        return deviceForUID((__bridge CFStringRef)value) != kAudioObjectUnknown;
    }
}
int cb_driver_available(void) { return deviceForUID(SEND_UID) != kAudioObjectUnknown; }

static OSStatus render(AudioObjectID device, const AudioTimeStamp *now,
                       const AudioBufferList *input, const AudioTimeStamp *inputTime,
                       AudioBufferList *output, const AudioTimeStamp *outputTime, void *context) {
    Engine *e = context;
    // Clear every output, including unused hardware outputs, before any early exit.
    for (UInt32 i = 0; i < output->mNumberBuffers; i++)
        if (output->mBuffers[i].mData) memset(output->mBuffers[i].mData, 0, output->mBuffers[i].mDataByteSize);
    if (atomic_load_explicit(&e->invalidated, memory_order_relaxed)) return noErr;
    if (!input || input->mNumberBuffers > MAX_BUFFERS || output->mNumberBuffers > MAX_BUFFERS) {
        atomic_fetch_add_explicit(&e->faults, 1, memory_order_relaxed); return noErr;
    }
    CBBuffer in[MAX_BUFFERS], out[MAX_BUFFERS];
    for (UInt32 i = 0; i < input->mNumberBuffers; i++) {
        AudioBuffer b = input->mBuffers[i];
        in[i] = (CBBuffer){b.mData, b.mNumberChannels, b.mNumberChannels ? b.mDataByteSize / (4 * b.mNumberChannels) : 0};
    }
    for (UInt32 i = 0; i < output->mNumberBuffers; i++) {
        AudioBuffer b = output->mBuffers[i];
        out[i] = (CBBuffer){b.mData, b.mNumberChannels, b.mNumberChannels ? b.mDataByteSize / (4 * b.mNumberChannels) : 0};
    }
    float l, r;
    if (!cb_mix(in, input->mNumberBuffers, out, output->mNumberBuffers,
                e->left, e->right, e->mode, e->gain, e->muted, &l, &r))
        atomic_fetch_add_explicit(&e->faults, 1, memory_order_relaxed);
    // Integer meters are lock-free on both supported architectures.
    atomic_store_explicit(&e->peakL, (uint32_t)(l * 10000), memory_order_relaxed);
    atomic_store_explicit(&e->peakR, (uint32_t)(r * 10000), memory_order_relaxed);
    atomic_fetch_add_explicit(&e->callbacks, 1, memory_order_relaxed);
    return noErr;
}

// Reject layouts we cannot safely interpret; never guess the sample format.
static BOOL floatStreams(AudioDeviceID device, AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress a = address(kAudioDevicePropertyStreams, scope);
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &a, 0, NULL, &size) || !size) return NO;
    AudioStreamID *streams = malloc(size);
    if (!streams) return NO;
    BOOL valid = AudioObjectGetPropertyData(device, &a, 0, NULL, &size, streams) == noErr;
    for (UInt32 i = 0; valid && i < size / sizeof(AudioStreamID); i++) {
        AudioStreamBasicDescription format = {0}; UInt32 n = sizeof(format);
        valid = get(streams[i], kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal, &n, &format) == noErr
            && format.mFormatID == kAudioFormatLinearPCM && format.mBitsPerChannel == 32
            && (format.mFormatFlags & kAudioFormatFlagIsFloat)
            && !(format.mFormatFlags & kAudioFormatFlagIsBigEndian)
            && (format.mFormatFlags & kAudioFormatFlagIsPacked)
            && format.mBytesPerFrame == 4 * ((format.mFormatFlags & kAudioFormatFlagIsNonInterleaved) ? 1 : format.mChannelsPerFrame);
    }
    free(streams); return valid;
}
// Listeners only invalidate the real-time route. Rebuilding happens on the
// control thread, never inside HAL's property notification callback.
static OSStatus changed(AudioObjectID object, UInt32 count,
                        const AudioObjectPropertyAddress *properties, void *context) {
    Engine *e = context;
    atomic_store_explicit(&e->invalidated, 1, memory_order_relaxed);
    return noErr;
}
typedef struct { AudioObjectID object; AudioObjectPropertyAddress property; } Watch;
static Watch watches[6];
static OSStatus watchChanges(void) {
    watches[0] = (Watch){engine.source, address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)};
    watches[1] = (Watch){engine.source, address(kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal)};
    watches[2] = (Watch){engine.source, address(kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeInput)};
    watches[3] = (Watch){engine.sink, address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)};
    watches[4] = (Watch){engine.aggregate, address(kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeInput)};
    watches[5] = (Watch){engine.aggregate, address(kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeOutput)};
    for (int i = 0; i < 6; i++) {
        OSStatus status = AudioObjectAddPropertyListener(watches[i].object, &watches[i].property, changed, &engine);
        if (status) return status;
        engine.listenerCount++;
    }
    return noErr;
}
void cb_stop(void) {
    atomic_store(&engine.invalidated, 1);
    for (int i = 0; i < engine.listenerCount; i++)
        AudioObjectRemovePropertyListener(watches[i].object, &watches[i].property, changed, &engine);
    engine.listenerCount = 0;
    if (engine.proc) {
        AudioDeviceStop(engine.aggregate, engine.proc);
        AudioDeviceDestroyIOProcID(engine.aggregate, engine.proc);
        engine.proc = NULL;
    }
    if (engine.aggregate) AudioHardwareDestroyAggregateDevice(engine.aggregate);
    engine.aggregate = 0;
}
// Aggregate channel ordering is part of our routing contract. Check the
// published order and first stream shape before any I/O is enabled.
static BOOL orderedLayout(NSString *sourceUID) {
    CFArrayRef uids = NULL; UInt32 size = sizeof(uids);
    if (get(engine.aggregate, kAudioAggregateDevicePropertyFullSubDeviceList,
            kAudioObjectPropertyScopeGlobal, &size, &uids) || !uids) return NO;
    NSArray *list = CFBridgingRelease(uids);
    if (list.count != 2 || ![list[0] isEqual:(__bridge NSString *)SEND_UID] || ![list[1] isEqual:sourceUID]) return NO;
    AudioObjectPropertyScope scopes[] = {kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput};
    for (int i = 0; i < 2; i++) {
        AudioObjectPropertyAddress a = address(kAudioDevicePropertyStreamConfiguration, scopes[i]);
        size = 0;
        if (AudioObjectGetPropertyDataSize(engine.aggregate, &a, 0, NULL, &size) || !size) return NO;
        AudioBufferList *buffers = calloc(1, size);
        if (!buffers) return NO;
        BOOL valid = AudioObjectGetPropertyData(engine.aggregate, &a, 0, NULL, &size, buffers) == noErr
            && buffers->mNumberBuffers > 0 && buffers->mBuffers[0].mNumberChannels == 2;
        free(buffers);
        if (!valid) return NO;
    }
    return YES;
}
// Disable the virtual device's input (avoid feedback) and every physical output.
// We only capture hardware input and write the first, virtual, output stream.
static OSStatus setStreamUsage(AudioObjectPropertyScope scope) {
    AudioObjectPropertyAddress a = address(kAudioDevicePropertyStreams, scope);
    UInt32 bytes = 0;
    OSStatus status = AudioObjectGetPropertyDataSize(engine.aggregate, &a, 0, NULL, &bytes);
    if (status || bytes < sizeof(AudioStreamID)) return status ?: -4;
    UInt32 count = bytes / sizeof(AudioStreamID);
    UInt32 size = (UInt32)(offsetof(AudioHardwareIOProcStreamUsage, mStreamIsOn) + count * sizeof(UInt32));
    AudioHardwareIOProcStreamUsage *usage = calloc(1, size);
    if (!usage) return memFullErr;
    usage->mIOProc = (void *)engine.proc; usage->mNumberStreams = count;
    for (UInt32 i = 0; i < count; i++) usage->mStreamIsOn[i] = scope == kAudioObjectPropertyScopeOutput ? i == 0 : i != 0;
    a = address(kAudioDevicePropertyIOProcStreamUsage, scope);
    status = AudioObjectSetPropertyData(engine.aggregate, &a, 0, NULL, size, usage);
    free(usage); return status;
}
static char *failure(NSString *message, OSStatus status) {
    cb_stop();
    return strdup([NSString stringWithFormat:@"%@ (Core Audio: %d)", message, (int)status].UTF8String);
}
char *cb_start(const char *uid, int left, int right, int mode, float gain, int muted) {
    @autoreleasepool {
        cb_stop();
        NSString *sourceUID = [NSString stringWithUTF8String:uid];
        AudioDeviceID source = deviceForUID((__bridge CFStringRef)sourceUID);
        AudioDeviceID sink = deviceForUID(SEND_UID);
        uint32_t count = channels(source, kAudioObjectPropertyScopeInput);
        if (!source || !sink || left < 0 || (uint32_t)left >= count || right < 0 || (uint32_t)right >= count)
            return failure(@"入力または仮想マイクを確認してください", -1);
        Float64 rate = sampleRate(source);
        AudioObjectPropertyAddress rateAddress = address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
        // Only the bundled virtual device changes rate. The hardware keeps its existing clock.
        OSStatus status = AudioObjectSetPropertyData(sink, &rateAddress, 0, NULL, sizeof(rate), &rate);
        if (status) return failure(@"仮想マイクのサンプルレートを設定できません", status);
        // Driver rate changes are asynchronous. Wait outside the real-time thread.
        for (int i = 0; i < 100 && sampleRate(sink) != rate; i++) [NSThread sleepForTimeInterval:0.01];
        if (sampleRate(sink) != rate) return failure(@"サンプルレートの変更が完了しません", -2);
        NSDictionary *composition = @{
            @kAudioAggregateDeviceNameKey: @"ChannelBridge Route",
            @kAudioAggregateDeviceUIDKey: [@"local.channelbridge.route." stringByAppendingString:NSUUID.UUID.UUIDString],
            @kAudioAggregateDeviceIsPrivateKey: @YES,
            @kAudioAggregateDeviceIsStackedKey: @NO,
            @kAudioAggregateDeviceMainSubDeviceKey: sourceUID,
            // Virtual device comes first: output 1/2 go to the mic; input 1/2 are disabled.
            @kAudioAggregateDeviceSubDeviceListKey: @[
                @{@kAudioSubDeviceUIDKey: (__bridge NSString *)SEND_UID,
                  @kAudioSubDeviceDriftCompensationKey: @YES,
                  @kAudioSubDeviceDriftCompensationQualityKey: @0x7F},
                @{@kAudioSubDeviceUIDKey: sourceUID, @kAudioSubDeviceDriftCompensationKey: @NO}
            ]
        };
        status = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)composition, &engine.aggregate);
        if (status) return failure(@"内部オーディオ経路を作成できません", status);
        engine.source = source; engine.sink = sink; engine.sourceRate = rate; engine.inputChannels = count;
        // HAL may need a moment to publish the aggregate's streams.
        for (int i = 0; i < 100 && channels(engine.aggregate, kAudioObjectPropertyScopeInput) != count + 2; i++)
            [NSThread sleepForTimeInterval:0.01];
        if (channels(engine.aggregate, kAudioObjectPropertyScopeInput) != count + 2 ||
            channels(engine.aggregate, kAudioObjectPropertyScopeOutput) != channels(source, kAudioObjectPropertyScopeOutput) + 2 ||
            !orderedLayout(sourceUID) ||
            !floatStreams(engine.aggregate, kAudioObjectPropertyScopeInput) || !floatStreams(engine.aggregate, kAudioObjectPropertyScopeOutput))
            return failure(@"未対応のオーディオ形式、またはチャンネル構成です", -3);
        engine.left = left + 2; engine.right = right + 2; engine.mode = mode; engine.gain = gain; engine.muted = muted;
        atomic_store(&engine.peakL, 0); atomic_store(&engine.peakR, 0);
        atomic_store(&engine.callbacks, 0); atomic_store(&engine.faults, 0);
        status = AudioDeviceCreateIOProcID(engine.aggregate, render, &engine, &engine.proc);
        if (status) return failure(@"音声処理を準備できません", status);
        status = setStreamUsage(kAudioObjectPropertyScopeInput);
        if (!status) status = setStreamUsage(kAudioObjectPropertyScopeOutput);
        if (status) return failure(@"使用する音声ストリームを設定できません", status);
        atomic_store(&engine.invalidated, 0);
        status = watchChanges();
        if (status) return failure(@"入力の変更監視を開始できません", status);
        status = AudioDeviceStart(engine.aggregate, engine.proc);
        if (status) return failure(@"音声入力を開始できません。マイク権限を確認してください", status);
        return NULL;
    }
}
void cb_levels(float *left, float *right, uint64_t *callbacks, uint64_t *faults) {
    *left = atomic_load(&engine.peakL) / 10000.0f; *right = atomic_load(&engine.peakR) / 10000.0f;
    *callbacks = atomic_load(&engine.callbacks); *faults = atomic_load(&engine.faults);
}
int cb_healthy(void) {
    if (!engine.aggregate || atomic_load(&engine.invalidated)) return 0;
    UInt32 alive = 0, size = sizeof(alive);
    if (get(engine.source, kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, &size, &alive) || !alive) return 0;
    return sampleRate(engine.source) == engine.sourceRate && sampleRate(engine.sink) == engine.sourceRate
        && channels(engine.source, kAudioObjectPropertyScopeInput) == engine.inputChannels;
}

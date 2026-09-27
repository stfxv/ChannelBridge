// SPDX-License-Identifier: GPL-3.0-only
// These compile-time settings create our own duplex microphone; the unused mirror has no streams.
#define kDriver_Name "ChannelBridge"
#define kPlugIn_BundleID "local.channelbridge.driver"
#define kPlugIn_Icon ""
#define kHas_Driver_Name_Format false
#define kDevice_Name "ChannelBridge Mic"
#define kDevice2_Name "ChannelBridge Send"
#define kManufacturer_Name "ChannelBridge (based on BlackHole)"
#define kNumber_Of_Channels 2
#define kDevice_IsHidden false
#define kDevice_HasInput true
#define kDevice_HasOutput true
#define kDevice2_IsHidden true
#define kDevice2_HasInput false
#define kDevice2_HasOutput false
#define kEnableVolumeControl false
// Include common RME rates, including 32 kHz, absent from the upstream default.
#define kSampleRates 8000, 16000, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 128000, 176400, 192000, 352800, 384000, 705600, 768000

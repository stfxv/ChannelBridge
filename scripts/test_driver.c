// Exercises the actual driver code in a fake HAL host; does not install a driver.
#include <assert.h>
#include <stdio.h>
#include "ChannelBridge.c"

static dispatch_semaphore_t completed;
static OSStatus copyStorage(AudioServerPlugInHostRef host, CFStringRef key, CFPropertyListRef *data) {
    *data = NULL; return noErr;
}
static OSStatus notify(AudioServerPlugInHostRef host, AudioObjectID id, UInt32 n, const AudioObjectPropertyAddress *a) { return noErr; }
static OSStatus configure(AudioServerPlugInHostRef host, AudioObjectID id, UInt64 action, void *info) {
    // HAL stops all I/O on the one duplex device before changing rate.
    assert(id == kObjectID_Device);
    assert(BlackHole_StopIO(gAudioServerPlugInDriverRef, id, 1) == noErr);
    assert(BlackHole_StopIO(gAudioServerPlugInDriverRef, id, 2) == noErr);
    assert(BlackHole_PerformDeviceConfigurationChange(gAudioServerPlugInDriverRef, id, action, info) == noErr);
    assert(BlackHole_StartIO(gAudioServerPlugInDriverRef, id, 1) == noErr);
    assert(BlackHole_StartIO(gAudioServerPlugInDriverRef, id, 2) == noErr);
    dispatch_semaphore_signal(completed); return noErr;
}
static void checkAudio(void) {
    float sent[128], received[128];
    for (int i=0; i<128; i++) {sent[i]=(i%2 ? -.25f : .5f); received[i]=0;}
    AudioServerPlugInIOCycleInfo cycle = {0};
    cycle.mOutputTime.mSampleTime=128; cycle.mInputTime.mSampleTime=128; cycle.mCurrentTime.mSampleTime=0;
    assert(BlackHole_DoIOOperation(gAudioServerPlugInDriverRef,kObjectID_Device,kObjectID_Stream_Output,1,
        kAudioServerPlugInIOOperationWriteMix,64,&cycle,sent,NULL)==noErr);
    // A later write ensures the read's frames are available, as in HAL scheduling.
    cycle.mOutputTime.mSampleTime=256;
    assert(BlackHole_DoIOOperation(gAudioServerPlugInDriverRef,kObjectID_Device,kObjectID_Stream_Output,1,
        kAudioServerPlugInIOOperationWriteMix,64,&cycle,sent,NULL)==noErr);
    assert(BlackHole_DoIOOperation(gAudioServerPlugInDriverRef,kObjectID_Device,kObjectID_Stream_Input,2,
        kAudioServerPlugInIOOperationReadInput,64,&cycle,received,NULL)==noErr);
    for (int i=0;i<128;i++) assert(sent[i]==received[i]);
}
int main(void) {
    AudioServerPlugInHostInterface host={.CopyFromStorage=copyStorage,.PropertiesChanged=notify,.RequestDeviceConfigurationChange=configure};
    assert(BlackHole_Initialize(gAudioServerPlugInDriverRef,&host)==noErr);
    CFStringRef uid=CFSTR("ChannelBridge_UID"); AudioObjectID device=0; UInt32 bytes=0;
    AudioObjectPropertyAddress a={kAudioPlugInPropertyTranslateUIDToDevice,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
    assert(BlackHole_GetPropertyData(gAudioServerPlugInDriverRef,kObjectID_PlugIn,0,&a,sizeof(uid),&uid,sizeof(device),&bytes,&device)==noErr);
    assert(device==kObjectID_Device);
    assert(BlackHole_GetPropertyData(gAudioServerPlugInDriverRef,kObjectID_PlugIn,0,&a,0,&uid,sizeof(device),&bytes,&device)!=noErr);
    assert(BlackHole_StartIO(gAudioServerPlugInDriverRef,kObjectID_Device,1)==noErr);
    assert(BlackHole_StartIO(gAudioServerPlugInDriverRef,kObjectID_Device,2)==noErr);
    completed=dispatch_semaphore_create(0);
    double rates[]={44100,48000,96000,32000,192000,44100};
    for (unsigned i=0;i<sizeof(rates)/sizeof(rates[0]);i++) {
        a=(AudioObjectPropertyAddress){kAudioDevicePropertyNominalSampleRate,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
        assert(BlackHole_SetPropertyData(gAudioServerPlugInDriverRef,kObjectID_Device,0,&a,0,NULL,sizeof(double),&rates[i])==noErr);
        assert(dispatch_semaphore_wait(completed,dispatch_time(DISPATCH_TIME_NOW,2*NSEC_PER_SEC))==0);
        double actual=0;
        assert(BlackHole_GetPropertyData(gAudioServerPlugInDriverRef,kObjectID_Device,0,&a,0,NULL,sizeof(actual),&bytes,&actual)==noErr);
        assert(actual==rates[i]);
        for (int stream=kObjectID_Stream_Input;stream<=kObjectID_Stream_Output;stream+=3) {
            AudioStreamBasicDescription format={0};
            a=(AudioObjectPropertyAddress){kAudioStreamPropertyVirtualFormat,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
            assert(BlackHole_GetPropertyData(gAudioServerPlugInDriverRef,stream,0,&a,0,NULL,sizeof(format),&bytes,&format)==noErr);
            assert(format.mSampleRate==rates[i] && format.mChannelsPerFrame==2);
        }
        checkAudio();
    }
    assert(BlackHole_StopIO(gAudioServerPlugInDriverRef,kObjectID_Device,1)==noErr);
    assert(BlackHole_StopIO(gAudioServerPlugInDriverRef,kObjectID_Device,2)==noErr);
    puts("Driver: UID, six live rate transitions, input/output formats, audio loopback passed (fake HAL)");
}

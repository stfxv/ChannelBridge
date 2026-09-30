// Test-only ABI oracle, compiled against Apple's SDK. Not linked into the app.
#include <CoreAudio/AudioServerPlugIn.h>
#include <stddef.h>
#include <stdio.h>
int main(void) {
    printf("AudioTimestamp %zu\n", sizeof(AudioTimeStamp));
    printf("IoCycleInfo %zu\n", sizeof(AudioServerPlugInIOCycleInfo));
    printf("IoCycleInfo.current_time %zu\n", offsetof(AudioServerPlugInIOCycleInfo, mCurrentTime));
    printf("IoCycleInfo.output_time %zu\n", offsetof(AudioServerPlugInIOCycleInfo, mOutputTime));
    printf("ClientInfo %zu\n", sizeof(AudioServerPlugInClientInfo));
    printf("ClientInfo.bundle_id %zu\n", offsetof(AudioServerPlugInClientInfo, mBundleID));
    printf("PropertyAddress %zu\n", sizeof(AudioObjectPropertyAddress));
    printf("HostInterface %zu\n", sizeof(AudioServerPlugInHostInterface));
    printf("DriverInterface %zu\n", sizeof(AudioServerPlugInDriverInterface));
    printf("DriverInterface.do_io %zu\n", offsetof(AudioServerPlugInDriverInterface, DoIOOperation));
    printf("StreamFormat %zu\n", sizeof(AudioStreamBasicDescription));
    printf("RangedFormat %zu\n", sizeof(AudioStreamRangedDescription));
    printf("AudioBuffer %zu\n", sizeof(AudioBuffer));
    printf("AudioBufferList %zu\n", sizeof(AudioBufferList));
    printf("AudioBufferList.buffers %zu\n", offsetof(AudioBufferList, mBuffers));
    return 0;
}

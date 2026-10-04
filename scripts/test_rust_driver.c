// A fake HAL using Apple's real C interface to call the Rust dynamic library.
// No driver installation, physical device or microphone capture is involved.
#include <CoreAudio/AudioServerPlugIn.h>
#include <assert.h>
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#ifdef CHANNELBRIDGE_PRODUCTION
#define EXPECTED_UID CFSTR("ChannelBridge_UID")
#define EXPECTED_NAME CFSTR("ChannelBridge Mic")
#else
#define EXPECTED_UID CFSTR("ChannelBridge_Rust_Test_UID")
#define EXPECTED_NAME CFSTR("ChannelBridge Mic (Rust Test)")
#endif
enum { Plugin = 1, Device = 2, Input = 3, Output = 4 };
static _Atomic UInt64 pending_action;
static OSStatus changed(AudioServerPlugInHostRef h, AudioObjectID id, UInt32 n, const AudioObjectPropertyAddress *a) { return 0; }
static OSStatus copy_storage(AudioServerPlugInHostRef h, CFStringRef key, CFPropertyListRef *out) { *out = NULL; return 0; }
static OSStatus write_storage(AudioServerPlugInHostRef h, CFStringRef key, CFPropertyListRef value) { return 0; }
static OSStatus delete_storage(AudioServerPlugInHostRef h, CFStringRef key) { return 0; }
static OSStatus request_change(AudioServerPlugInHostRef h, AudioObjectID id, UInt64 action, void *info) {
    assert(id == Device); atomic_store(&pending_action, action); return 0;
}
static AudioObjectPropertyAddress address(UInt32 selector) {
    return (AudioObjectPropertyAddress){selector, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
}
static AudioServerPlugInDriverRef driver;
static UInt32 get_u32(AudioObjectID id, UInt32 selector) {
    AudioObjectPropertyAddress a=address(selector); UInt32 value=0, size=0;
    assert((*driver)->GetPropertyData(driver,id,0,&a,0,NULL,sizeof(value),&size,&value)==0);
    assert(size==sizeof(value)); return value;
}
static double get_rate(void) {
    AudioObjectPropertyAddress a=address(kAudioDevicePropertyNominalSampleRate); double rate=0; UInt32 size=0;
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(rate),&size,&rate)==0);
    assert(size==sizeof(rate)); return rate;
}
static void transfer(UInt32 operation, double time, UInt32 frames, float *audio) {
    AudioServerPlugInIOCycleInfo cycle={0};
    cycle.mInputTime.mSampleTime=time; cycle.mInputTime.mFlags=kAudioTimeStampSampleTimeValid;
    cycle.mOutputTime=cycle.mInputTime;
    assert((*driver)->DoIOOperation(driver,Device,operation==kAudioServerPlugInIOOperationReadInput?Input:Output,
           operation==kAudioServerPlugInIOOperationReadInput?2:1,operation,frames,&cycle,audio,NULL)==0);
}
static void rate_change(double rate) {
    AudioObjectPropertyAddress a=address(kAudioDevicePropertyNominalSampleRate);
    double before=get_rate();
    assert((*driver)->SetPropertyData(driver,Device,0,&a,0,NULL,sizeof(rate),&rate)==0);
    for (int i=0;i<2000 && atomic_load(&pending_action)==0;i++) usleep(1000);
    UInt64 action=atomic_exchange(&pending_action,0);
    assert(action==(UInt64)rate);
    assert(get_rate()==before); // Requested is not yet committed.
    assert((*driver)->PerformDeviceConfigurationChange(driver,Device,action,NULL)!=0); // Still running.
    assert((*driver)->StopIO(driver,Device,1)==0);
    assert((*driver)->StopIO(driver,Device,2)==0);
    // A rejected Perform consumes the request. The host need not call Abort;
    // a new asynchronous request must be accepted after stopping I/O.
    assert((*driver)->PerformDeviceConfigurationChange(driver,Device,action,NULL)!=0);
    assert((*driver)->SetPropertyData(driver,Device,0,&a,0,NULL,sizeof(rate),&rate)==0);
    for (int i=0;i<2000 && atomic_load(&pending_action)==0;i++) usleep(1000);
    action=atomic_exchange(&pending_action,0);
    assert(action==(UInt64)rate);
    assert((*driver)->PerformDeviceConfigurationChange(driver,Device,action,NULL)==0);
    assert(get_rate()==rate);
    assert((*driver)->StartIO(driver,Device,1)==0);
    assert((*driver)->StartIO(driver,Device,2)==0);
    for (AudioObjectID stream=Input;stream<=Output;stream++) {
        AudioStreamBasicDescription f={0}; UInt32 bytes=0;
        a=address(kAudioStreamPropertyVirtualFormat);
        assert((*driver)->GetPropertyData(driver,stream,0,&a,0,NULL,sizeof(f),&bytes,&f)==0);
        assert(f.mSampleRate==rate && f.mChannelsPerFrame==2 && f.mBitsPerChannel==32);
        assert(f.mFormatFlags==(kAudioFormatFlagIsFloat|kAudioFormatFlagIsPacked));
    }
}
int main(int argc, char **argv) {
    assert(argc==2 || (argc==3 && strcmp(argv[2],"--bundle")==0));
    void *library=NULL; CFPlugInRef plugin=NULL;
    if(argc==3) {
        CFURLRef url=CFURLCreateFromFileSystemRepresentation(NULL,(const UInt8 *)argv[1],strlen(argv[1]),true);
        assert(url); plugin=CFPlugInCreate(NULL,url); CFRelease(url); assert(plugin);
        CFArrayRef factories=CFPlugInFindFactoriesForPlugInTypeInPlugIn(kAudioServerPlugInTypeUUID,plugin);
        assert(factories && CFArrayGetCount(factories)==1);
        driver=CFPlugInInstanceCreate(NULL,(CFUUIDRef)CFArrayGetValueAtIndex(factories,0),kAudioServerPlugInTypeUUID);
        CFRelease(factories);
    } else {
        library=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);
        if(!library) { fprintf(stderr,"dlopen: %s\n",dlerror()); return 1; }
        void *(*factory)(CFAllocatorRef,CFUUIDRef)=dlsym(library,"ChannelBridge_Create");
        assert(factory);
        assert(factory(NULL,NULL)==NULL);
        assert(factory(NULL,kAudioServerPlugInDriverInterfaceUUID)==NULL);
        driver=factory(NULL,kAudioServerPlugInTypeUUID);
    }
    assert(driver);
    void *queried=NULL;
    assert((*driver)->QueryInterface(driver,CFUUIDGetUUIDBytes(kAudioServerPlugInDriverInterfaceUUID),&queried)==0 && queried==driver);
    assert((*driver)->Release(driver)==1);
    AudioServerPlugInHostInterface host={changed,copy_storage,write_storage,delete_storage,request_change};
    assert((*driver)->Initialize(driver,&host)==0);

    assert(get_u32(Plugin,kAudioObjectPropertyClass)==kAudioPlugInClassID);
    AudioObjectPropertyAddress a=address(kAudioPlugInPropertyDeviceList); UInt32 bytes=0;
    assert((*driver)->GetPropertyDataSize(driver,Plugin,0,&a,0,NULL,&bytes)==0 && bytes==4);
    assert(get_u32(Plugin,kAudioPlugInPropertyDeviceList)==Device);
    assert(get_u32(Input,kAudioStreamPropertyDirection)==1);
    assert(get_u32(Output,kAudioStreamPropertyDirection)==0);
    a=address(kAudioObjectPropertyCustomPropertyInfoList);
    AudioServerPlugInCustomPropertyInfo custom[2]={{0}};
    assert((*driver)->GetPropertyDataSize(driver,Device,0,&a,0,NULL,&bytes)==0 && bytes==sizeof(custom));
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(custom),&bytes,&custom)==0);
    assert(custom[0].mSelector=='cbst' && custom[0].mPropertyDataType==kAudioServerPlugInCustomPropertyDataTypeCFPropertyList && custom[0].mQualifierDataType==0);
    assert(custom[1].mSelector=='cbid' && custom[1].mPropertyDataType==kAudioServerPlugInCustomPropertyDataTypeCFString && custom[1].mQualifierDataType==0);
    a=address('cbid'); CFStringRef identity=NULL;
    assert((*driver)->GetPropertyDataSize(driver,Device,0,&a,0,NULL,&bytes)==0 && bytes==sizeof(identity));
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(identity),&bytes,&identity)==0);
    assert(CFEqual(identity,CFSTR("0.5.4"))); CFRelease(identity);
    a=address(custom[0].mSelector); UInt64 diagnostics[6]={0}; CFDataRef data=NULL;
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(data),&bytes,&data)==0 && bytes==sizeof(data));
    assert(CFGetTypeID(data)==CFDataGetTypeID() && CFDataGetLength(data)==48);
    memcpy(diagnostics,CFDataGetBytePtr(data),48); CFRelease(data);
    UInt32 protocol=0; memcpy(&protocol,diagnostics,sizeof(protocol)); assert(protocol==1);
    CFStringRef uid=EXPECTED_UID; UInt32 id=0;
    a=address(kAudioPlugInPropertyTranslateUIDToDevice);
    assert((*driver)->GetPropertyData(driver,Plugin,0,&a,sizeof(uid),&uid,sizeof(id),&bytes,&id)==0 && id==Device);
    assert((*driver)->GetPropertyData(driver,Plugin,0,&a,0,NULL,sizeof(id),&bytes,&id)!=0);
    assert((*driver)->GetPropertyData(driver,Plugin,0,&a,sizeof(uid),&uid,1,&bytes,&id)!=0);
    a=address(kAudioObjectPropertyName); CFStringRef name=NULL;
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(name),&bytes,&name)==0);
    assert(CFEqual(name,EXPECTED_NAME)); CFRelease(name);

    AudioServerPlugInClientInfo client={1,1234,true,NULL};
    assert((*driver)->AddDeviceClient(driver,Device,&client)==0);
    client.mClientID=2; assert((*driver)->AddDeviceClient(driver,Device,&client)==0);
    assert((*driver)->StartIO(driver,Device,1)==0);
    assert((*driver)->StartIO(driver,Device,2)==0);
    assert((*driver)->StartIO(driver,Device,2)!=0);
    UInt64 seed, ticks; double sample;
    assert((*driver)->GetZeroTimeStamp(driver,Device,1,&sample,&ticks,&seed)==0);
    UInt64 original_seed=seed;

    float sent[512], received[512];
    for(int i=0;i<512;i++) sent[i]=(i%2)?-.5f:.25f;
    transfer(kAudioServerPlugInIOOperationWriteMix,128.25,255,sent);
    transfer(kAudioServerPlugInIOOperationReadInput,128.5,256,received);
    for(int i=0;i<510;i++) assert(received[i]==sent[i]);
    assert(received[510]==0 && received[511]==0);
    a=address('cbst');
    data=NULL;
    assert((*driver)->GetPropertyData(driver,Device,0,&a,0,NULL,sizeof(data),&bytes,&data)==0 && bytes==sizeof(data));
    assert(CFGetTypeID(data)==CFDataGetTypeID() && CFDataGetLength(data)==48);
    memcpy(diagnostics,CFDataGetBytePtr(data),48); CFRelease(data);
    UInt32 reserved=1; memcpy(&reserved,(char *)diagnostics+4,sizeof(reserved));
    assert(reserved==0);
    assert(diagnostics[1]==original_seed && diagnostics[2]==255 && diagnostics[3]==1);
    assert(diagnostics[4]==0 && diagnostics[5]==0);
    transfer(kAudioServerPlugInIOOperationReadInput,400,256,received);
    transfer(kAudioServerPlugInIOOperationReadInput,128,256,received);
    assert(received[0]==.25f && received[1]==-.5f); // No global ring clearing.
    for(UInt64 frame=1024;frame<1024+65536*3;frame+=256) {
        transfer(kAudioServerPlugInIOOperationWriteMix,frame,256,sent);
        transfer(kAudioServerPlugInIOOperationReadInput,frame,256,received);
        for(int i=0;i<512;i++) assert(received[i]==sent[i]);
    }

    double rates[]={44100,48000,96000,32000,192000,768000,48000};
    for(unsigned i=0;i<sizeof(rates)/sizeof(rates[0]);i++) rate_change(rates[i]);
    assert((*driver)->GetZeroTimeStamp(driver,Device,1,&sample,&ticks,&seed)==0 && seed!=original_seed);
    transfer(kAudioServerPlugInIOOperationReadInput,128,256,received);
    for(int i=0;i<512;i++) assert(received[i]==0); // Old timeline cannot leak.

    assert((*driver)->StopIO(driver,Device,1)==0);
    assert(get_u32(Device,kAudioDevicePropertyDeviceIsRunning)==1);
    assert((*driver)->StopIO(driver,Device,2)==0);
    assert(get_u32(Device,kAudioDevicePropertyDeviceIsRunning)==0);
    assert((*driver)->StopIO(driver,Device,2)!=0);
    client.mClientID=1; assert((*driver)->RemoveDeviceClient(driver,Device,&client)==0);
    client.mClientID=2; assert((*driver)->RemoveDeviceClient(driver,Device,&client)==0);
    assert((*driver)->Release(driver)==0);
    if(library) assert(dlclose(library)==0);
    if(plugin) CFRelease(plugin);
    puts("Rust driver: real C factory/interface, properties, clients, partial audio, wrap, seven async rates and epochs passed (fake HAL)");
    return 0;
}

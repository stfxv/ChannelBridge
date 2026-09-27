#include <assert.h>
#include <stdio.h>
#include "../internal/native/mixer.h"
int main(void) {
    float hardware[4 * 32] = {0}, output[8] = {0}, unused[8] = {0};
    for (int f=0; f<4; f++) { hardware[f*32+8]=.25f; hardware[f*32+15]=-.5f; }
    CBBuffer in[]={{hardware,32,4}}, out[]={{output,2,4},{unused,2,4}};
    float l,r;
    assert(cb_mix(in,1,out,2,8,15,0,1,0,&l,&r));
    assert(output[0]==.25f && output[1]==.25f && unused[0]==0);
    assert(cb_mix(in,1,out,2,8,15,1,1,0,&l,&r));
    assert(output[0]==.25f && output[1]==-.5f);
    assert(cb_mix(in,1,out,2,8,15,2,1,0,&l,&r));
    assert(output[0]==-.125f && output[1]==-.125f);
    // Planar input and split output streams, gain clipping, NaN sanitization.
    float x[]={2,NAN}, y[]={-.4f,.1f}, a[2]={0}, b[2]={0};
    CBBuffer planar[]={{x,1,2},{y,1,2}}, split[]={{a,1,2},{b,1,2}};
    assert(cb_mix(planar,2,split,2,0,1,1,2,0,&l,&r));
    assert(a[0]==1 && a[1]==0 && b[0]==-.8f);
    assert(!cb_mix(planar,2,split,2,2,0,0,1,0,&l,&r));
    CBBuffer shortInput[]={{x,1,1}};
    assert(!cb_mix(shortInput,1,split,2,0,0,0,1,0,&l,&r));
    a[0]=a[1]=b[0]=b[1]=0;
    assert(cb_mix(planar,2,split,2,0,1,1,1,1,&l,&r));
    assert(a[0]==0 && b[0]==0 && l==0 && r==0);
    puts("Mixer: multichannel/planar/mono/stereo/mix/mute/clip/bounds passed");
}

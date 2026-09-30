#include <assert.h>
#include <stdio.h>
#include "../internal/native/mixer.h"
int main(void) {
    float hardware[4 * 32] = {0}, output[8] = {0}, unused[8] = {0};
    for (int f=0; f<4; f++) { hardware[f*32+8]=.25f; hardware[f*32+15]=-.5f; }
    CBBuffer in[]={{hardware,32,4}}, out[]={{output,2,4},{unused,2,4}};
    float l,r;
    assert(cb_mix(in,1,out,2,8,15,0,0,&l,&r));
    assert(output[0]==.25f && output[1]==.25f && unused[0]==0 && l==.25f && r==.25f);
    assert(cb_mix(in,1,out,2,8,15,1,0,&l,&r));
    assert(output[0]==.25f && output[1]==-.5f && l==.25f && r==.5f);
    assert(cb_mix(in,1,out,2,8,15,2,0,&l,&r));
    assert(output[0]==-.125f && output[1]==-.125f && l==.25f && r==.5f);
    // Planar input, split outputs, raw metering before clipping, NaN sanitization.
    float x[]={2,NAN}, y[]={-.4f,.1f}, a[2]={0}, b[2]={0};
    CBBuffer planar[]={{x,1,2},{y,1,2}}, split[]={{a,1,2},{b,1,2}};
    assert(cb_mix(planar,2,split,2,0,1,1,0,&l,&r));
    assert(a[0]==1 && a[1]==0 && b[0]==-.4f && l==2 && r==.4f);
    assert(!cb_mix(planar,2,split,2,2,0,0,0,&l,&r));
    CBBuffer shortInput[]={{x,1,1}};
    assert(!cb_mix(shortInput,1,split,2,0,0,0,0,&l,&r));
    a[0]=a[1]=b[0]=b[1]=0;
    assert(cb_mix(planar,2,split,2,0,1,1,1,&l,&r));
    assert(a[0]==0 && b[0]==0 && l==2 && r==.4f); // Muting never hides input activity.
    // Two equal/opposite inputs cancel at output, but both raw meters show signal.
    float pos[]={.8f}, neg[]={-.8f}, mixed[2]={0};
    CBBuffer cancel[]={{pos,1,1},{neg,1,1}}, mono[]={{mixed,2,1}};
    assert(cb_mix(cancel,2,mono,1,0,1,2,0,&l,&r));
    assert(mixed[0]==0 && mixed[1]==0 && l==.8f && r==.8f);
    // Input-only preview reads all frames independently of any output buffer.
    assert(cb_input_peaks(in,1,8,15,&l,&r));
    assert(l==.25f && r==.5f);
    assert(cb_input_peaks(planar,2,0,1,&l,&r));
    assert(l==2 && r==.4f);
    assert(cb_input_peaks(cancel,2,0,1,&l,&r));
    assert(l==.8f && r==.8f);
    assert(cb_input_peaks(planar,2,1,1,&l,&r));
    assert(l==.4f && r==.4f);
    assert(!cb_input_peaks(planar,2,2,0,&l,&r));
    CBBuffer missing[]={{NULL,1,2}}, empty[]={{x,1,0}};
    assert(!cb_input_peaks(missing,1,0,0,&l,&r));
    assert(!cb_input_peaks(empty,1,0,0,&l,&r));
    // The UI polls less often than audio callbacks: keep a transient until read.
    _Atomic uint32_t meter=0;
    cb_hold_peak(&meter,.7f); cb_hold_peak(&meter,.01f); cb_hold_peak(&meter,0);
    assert(cb_take_peak(&meter)==.7f && cb_take_peak(&meter)==0);
    cb_hold_peak(&meter,0.00001f); cb_hold_peak(&meter,NAN); cb_hold_peak(&meter,INFINITY);
    assert(cb_take_peak(&meter)==0.00001f); // -100 dBFS must not quantize to silence.
    // All-channel overview skips virtual aggregate inputs and tolerates disabled
    // buffers, while retaining transients independently on every physical input.
    _Atomic uint32_t all[32] = {0};
    cb_all_peaks(in, 1, 0, 32, all);
    assert(cb_take_peak(&all[8]) == .25f && cb_take_peak(&all[15]) == .5f);
    CBBuffer aggregate[] = {{NULL,2,4}, {hardware,32,4}};
    cb_all_peaks(aggregate, 2, 2, 32, all);
    assert(cb_take_peak(&all[8]) == .25f && cb_take_peak(&all[15]) == .5f);
    assert(cb_take_peak(&all[0]) == 0);
    cb_all_peaks(planar, 2, 0, 2, all);
    assert(cb_take_peak(&all[0]) == 2 && cb_take_peak(&all[1]) == .4f);
    cb_all_peaks(planar, 2, 0, 1, all);
    assert(cb_take_peak(&all[0]) == 2 && cb_take_peak(&all[1]) == 0);
    puts("Mixer: channel routing, raw input meters, mute, cancellation, transient hold and quiet levels passed");
}

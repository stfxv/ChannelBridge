// SPDX-License-Identifier: GPL-3.0-only
#import <Cocoa/Cocoa.h>
#import <AVFoundation/AVFoundation.h>
#include "bridge.h"

@interface CBApp : NSObject <NSApplicationDelegate, NSWindowDelegate>
@property NSWindow *window;
@property NSStatusItem *item;
@property NSPopUpButton *devices, *left, *right, *mode;
@property NSTextField *status, *format, *levelL, *levelR;
@property NSButton *start, *mute, *install;
@property NSLevelIndicator *meterL, *meterR;
@property NSDictionary *model;
@property NSString *deviceSignature;
@property BOOL updating, permissionRequested;
@property NSInteger lastAuthorization;
@end
static CBApp *delegate;

static NSTextField *label(NSString *text, NSRect frame, CGFloat size, NSColor *color) {
    NSTextField *v = [NSTextField labelWithString:text];
    v.frame = frame; v.font = [NSFont systemFontOfSize:size]; v.textColor = color; return v;
}
static NSButton *button(NSString *title, NSRect frame, id target, SEL action) {
    NSButton *b = [NSButton buttonWithTitle:title target:target action:action];
    b.frame = frame; b.bezelStyle = NSBezelStyleRounded; return b;
}
static NSPopUpButton *popup(NSRect frame, id target) {
    NSPopUpButton *p = [[NSPopUpButton alloc] initWithFrame:frame pullsDown:NO];
    p.target = target; p.action = @selector(changed:); return p;
}
static void action(NSString *name, NSDictionary *payload) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload ?: @{} options:0 error:nil];
    NSString *json = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    goAction((char *)name.UTF8String, (char *)json.UTF8String);
}
@implementation CBApp
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    self.lastAuthorization = -1;
    self.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 520, 470)
        styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
        backing:NSBackingStoreBuffered defer:NO];
    self.window.title = @"ChannelBridge"; self.window.delegate = self;
    self.window.releasedWhenClosed = NO;
    NSView *v = self.window.contentView;
    [v addSubview:label(@"入力デバイス", NSMakeRect(30, 432, 300, 20), 12, NSColor.secondaryLabelColor)];
    self.devices = popup(NSMakeRect(26, 394, 380, 32), self); [v addSubview:self.devices];
    [v addSubview:button(@"更新", NSMakeRect(414, 394, 78, 32), self, @selector(refresh:))];
    self.format = label(@"", NSMakeRect(30, 354, 462, 38), 12, NSColor.labelColor);
    self.format.maximumNumberOfLines = 2; self.format.lineBreakMode = NSLineBreakByWordWrapping;
    self.format.toolTip = @"サンプルレートと、macOSドライバが報告する選択チャンネルの入力形式。AD変換器の実効分解能とは異なる場合があります。";
    [v addSubview:self.format];
    [v addSubview:label(@"処理・仮想マイク: 32 bit float", NSMakeRect(30, 332, 462, 18), 11, NSColor.secondaryLabelColor)];
    [v addSubview:label(@"出力モード", NSMakeRect(30, 304, 250, 20), 12, NSColor.secondaryLabelColor)];
    self.mode = popup(NSMakeRect(26, 268, 466, 32), self);
    [self.mode addItemsWithTitles:@[@"モノラル — 1入力を左右へ", @"ステレオ — 2入力を左右へ", @"ミックス — 2入力を平均して左右へ"]]; [v addSubview:self.mode];
    [v addSubview:label(@"入力チャンネル L / モノ", NSMakeRect(30, 242, 225, 20), 12, NSColor.secondaryLabelColor)];
    [v addSubview:label(@"入力チャンネル R", NSMakeRect(275, 242, 210, 20), 12, NSColor.secondaryLabelColor)];
    self.left = popup(NSMakeRect(26, 208, 218, 32), self); self.right = popup(NSMakeRect(270, 208, 222, 32), self);
    [v addSubview:self.left]; [v addSubview:self.right];
    self.levelL = label(@"", NSMakeRect(30, 177, 224, 24), 16, NSColor.labelColor);
    self.levelR = label(@"", NSMakeRect(268, 177, 224, 24), 16, NSColor.labelColor);
    for (NSTextField *l in @[self.levelL, self.levelR]) {
        l.font = [NSFont monospacedDigitSystemFontOfSize:16 weight:NSFontWeightMedium];
        l.toolTip = @"選択した入力のピーク値（dBFS）。ミックス・ミュート・出力クリップ前に測定します。";
        [v addSubview:l];
    }
    self.meterL = [[NSLevelIndicator alloc] initWithFrame:NSMakeRect(30, 155, 224, 12)];
    self.meterR = [[NSLevelIndicator alloc] initWithFrame:NSMakeRect(268, 155, 224, 12)];
    for (NSLevelIndicator *m in @[self.meterL, self.meterR]) {
        m.minValue = -60; m.maxValue = 0; m.warningValue = -6; m.criticalValue = -1;
        m.levelIndicatorStyle = NSLevelIndicatorStyleContinuousCapacity; [v addSubview:m];
    }
    [v addSubview:label(@"入力ピーク: −60〜0 dBFS · 開始前・停止中も表示", NSMakeRect(30, 132, 462, 18), 11, NSColor.secondaryLabelColor)];
    self.mute = [NSButton checkboxWithTitle:@"送出をミュート" target:self action:@selector(changed:)];
    self.mute.frame = NSMakeRect(28, 101, 220, 26); [v addSubview:self.mute];
    self.status = label(@"", NSMakeRect(30, 56, 462, 38), 12, NSColor.secondaryLabelColor);
    self.status.maximumNumberOfLines = 2; self.status.lineBreakMode = NSLineBreakByWordWrapping; [v addSubview:self.status];
    self.install = button(@"仮想マイクをセットアップ", NSMakeRect(24, 14, 246, 34), self, @selector(install:)); [v addSubview:self.install];
    self.start = button(@"開始", NSMakeRect(376, 14, 120, 34), self, @selector(toggle:)); self.start.keyEquivalent = @"\r"; [v addSubview:self.start];
    self.item = [NSStatusBar.systemStatusBar statusItemWithLength:NSVariableStatusItemLength]; self.item.button.title = @"CB ○";
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@"ChannelBridge"];
    NSMenuItem *show = [menu addItemWithTitle:@"ChannelBridge を開く" action:@selector(show:) keyEquivalent:@""]; show.target = self;
    NSMenuItem *stop = [menu addItemWithTitle:@"送出を停止" action:@selector(stop:) keyEquivalent:@""]; stop.target = self;
    [menu addItem:NSMenuItem.separatorItem];
    NSMenuItem *quit = [menu addItemWithTitle:@"終了" action:@selector(terminate:) keyEquivalent:@"q"]; quit.target = NSApp;
    self.item.menu = menu;
    NSMenu *main = [[NSMenu alloc] init]; NSMenuItem *appMenu = [[NSMenuItem alloc] init]; [main addItem:appMenu];
    NSMenu *submenu = [[NSMenu alloc] init]; [submenu addItemWithTitle:@"ChannelBridge を終了" action:@selector(terminate:) keyEquivalent:@"q"]; appMenu.submenu = submenu; NSApp.mainMenu = main;
    [self update:self.model]; [self.window center]; [self show:nil];
    [self checkPermission];
    [NSTimer scheduledTimerWithTimeInterval:0.25 target:self selector:@selector(tick:) userInfo:nil repeats:YES];
}
- (void)show:(id)sender { [self.window makeKeyAndOrderFront:nil]; [NSApp activateIgnoringOtherApps:YES]; }
- (BOOL)applicationShouldHandleReopen:(NSApplication *)app hasVisibleWindows:(BOOL)flag { [self show:nil]; return YES; }
- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)app { return NO; }
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender { action(@"quit", nil); return NSTerminateNow; }
- (void)refresh:(id)sender { action(@"refresh", nil); }
// Metering requires microphone authorization even while sending is stopped.
- (void)checkPermission {
    if (![self.model[@"config"][@"device_uid"] length]) return;
    AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status == AVAuthorizationStatusNotDetermined) {
        if (self.permissionRequested) return;
        self.permissionRequested = YES;
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
            dispatch_async(dispatch_get_main_queue(), ^{ self.permissionRequested = NO; [self checkPermission]; });
        }];
        return;
    }
    if (self.lastAuthorization == status) return;
    self.lastAuthorization = status;
    action(status == AVAuthorizationStatusAuthorized ? @"permission_granted" : @"permission_denied", nil);
}
- (void)tick:(id)sender { [self checkPermission]; action(@"tick", nil); }
- (void)stop:(id)sender { action(@"stop", nil); }
- (void)toggle:(id)sender {
    if ([self.model[@"wanted"] boolValue]) { action(@"stop", nil); return; }
    AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status == AVAuthorizationStatusNotDetermined) {
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
            dispatch_async(dispatch_get_main_queue(), ^{ action(granted ? @"start" : @"permission_denied", nil); });
        }];
    } else action(status == AVAuthorizationStatusAuthorized ? @"start" : @"permission_denied", nil);
}
- (void)install:(id)sender {
    NSString *path = [NSBundle.mainBundle pathForResource:@"ChannelBridgeDriver" ofType:@"pkg"];
    if (path) [[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:path]];
    else { NSAlert *a = [[NSAlert alloc] init]; a.messageText = @"インストーラが見つかりません"; a.informativeText = @"ビルド済みの ChannelBridge.app から起動してください。"; [a runModal]; }
}
- (void)changed:(id)sender {
    if (self.updating) return;
    NSMutableDictionary *config = [self.model[@"config"] mutableCopy];
    NSString *uid = self.devices.selectedItem.representedObject;
    if (!uid) return;
    config[@"device_uid"] = uid;
    // An unplugged device has no channel menu; preserve its saved selection.
    if (self.left.selectedItem) config[@"left"] = @(self.left.selectedItem.tag);
    if (self.right.selectedItem) config[@"right"] = @(self.right.selectedItem.tag);
    if (sender == self.devices) { config[@"left"] = @1; config[@"right"] = @1; }
    config[@"mode"] = @[@"mono", @"stereo", @"mix"][self.mode.indexOfSelectedItem];
    config[@"muted"] = self.mute.state == NSControlStateValueOn ? @YES : @NO;
    action(@"config", config);
    [self checkPermission];
}
- (void)updateLevel:(NSTextField *)text meter:(NSLevelIndicator *)meter channel:(NSInteger)channel db:(double)db active:(BOOL)active {
    NSString *value = !active ? @"—" : (db <= -120 ? @"−∞ dBFS" : [NSString stringWithFormat:@"%.1f dBFS", db]);
    text.stringValue = [NSString stringWithFormat:@"Ch %ld · %@", (long)channel, value];
    text.textColor = active && db >= 0 ? NSColor.systemRedColor : NSColor.labelColor;
    meter.doubleValue = active ? fmax(-60, fmin(0, db)) : -60;
}
- (void)update:(NSDictionary *)model {
    self.model = model; if (!self.window) return;
    self.updating = YES;
    NSDictionary *config = model[@"config"]; NSString *uid = config[@"device_uid"];
    NSArray *devices = model[@"devices"];
    if (![devices isKindOfClass:NSArray.class]) devices = @[];
    // Don't rebuild menus on every meter tick: preserve an open popup interaction.
    NSString *signature = [NSString stringWithFormat:@"%@/%@", devices, uid];
    if (![signature isEqualToString:self.deviceSignature]) {
        self.deviceSignature = signature; [self.devices removeAllItems]; int count = 0;
        [self.devices addItemWithTitle:@"入力デバイスを選択…"];
        for (NSDictionary *d in devices) {
            [self.devices addItemWithTitle:[NSString stringWithFormat:@"%@ · %@ ch", d[@"name"], d[@"channels"]]];
            self.devices.lastItem.representedObject = d[@"uid"];
            if ([d[@"uid"] isEqual:uid]) { [self.devices selectItem:self.devices.lastItem]; count = [d[@"channels"] intValue]; }
        }
        if (uid.length && !count) { [self.devices addItemWithTitle:@"保存した入力（未接続）"]; self.devices.lastItem.representedObject = uid; [self.devices selectItem:self.devices.lastItem]; }
        [self.left removeAllItems]; [self.right removeAllItems];
        for (int i = 1; i <= count; i++) for (NSPopUpButton *p in @[self.left, self.right]) { [p addItemWithTitle:[NSString stringWithFormat:@"Ch %d", i]]; p.lastItem.tag = i; }
    }
    if (self.left.selectedItem.tag != [config[@"left"] integerValue]) [self.left selectItemWithTag:[config[@"left"] integerValue]];
    if (self.right.selectedItem.tag != [config[@"right"] integerValue]) [self.right selectItemWithTag:[config[@"right"] integerValue]];
    NSUInteger mode = [@[@"mono", @"stereo", @"mix"] indexOfObject:config[@"mode"]];
    if (mode == NSNotFound) mode = 0;
    if (self.mode.indexOfSelectedItem != mode) [self.mode selectItemAtIndex:mode];
    self.right.enabled = mode != 0;
    self.mute.state = [config[@"muted"] boolValue] ? NSControlStateValueOn : NSControlStateValueOff;
    self.status.stringValue = model[@"status"] ?: @"";
    self.format.stringValue = model[@"input_format"] ?: @"入力フォーマット: 未接続";
    BOOL active = [model[@"running"] boolValue] || [model[@"monitoring"] boolValue];
    double dbL = [model[@"level_left"] doubleValue], dbR = [model[@"level_right"] doubleValue];
    [self updateLevel:self.levelL meter:self.meterL channel:[config[@"left"] integerValue] db:dbL active:active];
    [self updateLevel:self.levelR meter:self.meterR channel:[config[@"right"] integerValue] db:dbR active:active];
    self.levelR.hidden = mode == 0; self.meterR.hidden = mode == 0;
    NSString *startTitle = [model[@"wanted"] boolValue] ? @"停止" : @"開始";
    if (![self.start.title isEqualToString:startTitle]) self.start.title = startTitle;
    self.start.enabled = [model[@"wanted"] boolValue] || ([model[@"driver"] boolValue] && uid.length > 0);
    self.install.hidden = [model[@"driver"] boolValue];
    self.item.button.title = [model[@"running"] boolValue] ? ([config[@"muted"] boolValue] ? @"CB −" : @"CB ●") : @"CB ○";
    self.updating = NO;
}
@end
void cb_run(const char *model) {
    @autoreleasepool {
        [NSApplication sharedApplication]; [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        // A single routing instance prevents duplicate summing into the virtual mic.
        NSArray *instances = [NSRunningApplication runningApplicationsWithBundleIdentifier:@"local.channelbridge.app"];
        for (NSRunningApplication *app in instances) if (app.processIdentifier != NSProcessInfo.processInfo.processIdentifier) {
            [app activateWithOptions:NSApplicationActivateIgnoringOtherApps]; return;
        }
        delegate = [[CBApp alloc] init];
        delegate.model = [NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:model] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
        NSApp.delegate = delegate; [NSApp run];
    }
}
void cb_present(const char *model) {
    @autoreleasepool {
        NSDictionary *value = [NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:model] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
        [delegate update:value];
    }
}

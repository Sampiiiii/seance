#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <QuartzCore/QuartzCore.h>
#import <objc/runtime.h>
#import <objc/message.h>
#include <stdbool.h>

// Opts the application's windows into ProMotion 120 Hz rendering on macOS 14+.
//
// GPUI's macOS backend draws into a CAMetalLayer. When the system hands AppKit
// a ProMotion display, the implicit display link driving that layer still runs
// at the screen's base rate (60 Hz) in fullscreen unless we set
// `preferredFrameRateRange` to request the higher cadence. We install
// notification observers so every window AppKit hands us gets the hint applied
// once it's main/key/fullscreen. All selectors are checked at runtime so older
// macOS versions silently fall through.

static BOOL SeancePromotionInstalled = NO;
static id<NSObject> SeancePromotionBecomeMainObserver = nil;
static id<NSObject> SeancePromotionBecomeKeyObserver = nil;
static id<NSObject> SeancePromotionEnterFullScreenObserver = nil;
static id<NSObject> SeancePromotionScreenChangedObserver = nil;
static id<NSObject> SeancePromotionAppLaunchedObserver = nil;

static BOOL SeanceScreenSupportsProMotion(NSScreen *screen) {
    if (screen == nil) {
        return NO;
    }
    if (![screen respondsToSelector:@selector(maximumFramesPerSecond)]) {
        return NO;
    }
    NSInteger maxFps = [screen maximumFramesPerSecond];
    return maxFps >= 100;
}

static void SeanceApplyPreferredFrameRate(CALayer *layer, NSScreen *screen) {
    if (layer == nil) {
        return;
    }
    if (![layer isKindOfClass:[CAMetalLayer class]]) {
        return;
    }

    // Best-effort maximum drawable count (default is 3, but set explicitly to
    // avoid any recent driver regressions).
    CAMetalLayer *metalLayer = (CAMetalLayer *)layer;
    @try {
        metalLayer.maximumDrawableCount = 3;
    } @catch (NSException *exception) {
        // Fall through - harmless.
    }

    // preferredFrameRateRange is a CAMetalDisplayLink / CADisplayLink property
    // on macOS 14+. Some builds expose it on the layer via private bridging;
    // probe reflectively and only set it if the runtime accepts the selector.
    SEL setPreferredRange = NSSelectorFromString(@"setPreferredFrameRateRange:");
    if ([layer respondsToSelector:setPreferredRange]) {
        float maxHz = 120.0f;
        if (screen != nil && [screen respondsToSelector:@selector(maximumFramesPerSecond)]) {
            NSInteger systemMax = [screen maximumFramesPerSecond];
            if (systemMax > 0 && systemMax < 120) {
                maxHz = (float)systemMax;
            }
        }
        struct SeanceFrameRateRange {
            float minimum;
            float maximum;
            float preferred;
        };
        struct SeanceFrameRateRange range = { 60.0f, maxHz, maxHz };
        NSMethodSignature *signature = [layer methodSignatureForSelector:setPreferredRange];
        if (signature != nil) {
            NSInvocation *invocation = [NSInvocation invocationWithMethodSignature:signature];
            [invocation setSelector:setPreferredRange];
            [invocation setTarget:layer];
            [invocation setArgument:&range atIndex:2];
            @try {
                [invocation invoke];
            } @catch (NSException *exception) {
                NSLog(@"[seance] preferredFrameRateRange invocation failed: %@", exception);
            }
        }
    }
}

static void SeanceApplyPromotionToWindow(NSWindow *window) {
    if (window == nil) {
        return;
    }
    NSScreen *screen = window.screen;
    if (!SeanceScreenSupportsProMotion(screen)) {
        return;
    }

    NSView *contentView = window.contentView;
    if (contentView == nil) {
        return;
    }

    // Walk the view hierarchy: GPUI may attach the CAMetalLayer on the content
    // view directly or on a child (e.g. a drawable host view).
    void (^visit)(NSView *) = ^(NSView *view) {
        if (view == nil) {
            return;
        }
        [view setWantsLayer:YES];
        SeanceApplyPreferredFrameRate(view.layer, screen);
    };

    visit(contentView);
    for (NSView *subview in contentView.subviews) {
        visit(subview);
        for (NSView *grandchild in subview.subviews) {
            visit(grandchild);
        }
    }
}

static void SeanceApplyPromotionToAllWindows(void) {
    for (NSWindow *window in [NSApp windows]) {
        SeanceApplyPromotionToWindow(window);
    }
}

static NSWindow *SeanceWindowFromNotification(NSNotification *notification) {
    id obj = notification.object;
    if ([obj isKindOfClass:[NSWindow class]]) {
        return (NSWindow *)obj;
    }
    return nil;
}

bool seance_promotion_install(void) {
    if (SeancePromotionInstalled) {
        return true;
    }

    dispatch_block_t installBlock = ^{
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];

        SeancePromotionBecomeMainObserver = [center
            addObserverForName:NSWindowDidBecomeMainNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) {
                        SeanceApplyPromotionToWindow(SeanceWindowFromNotification(notification));
                    }];

        SeancePromotionBecomeKeyObserver = [center
            addObserverForName:NSWindowDidBecomeKeyNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) {
                        SeanceApplyPromotionToWindow(SeanceWindowFromNotification(notification));
                    }];

        SeancePromotionEnterFullScreenObserver = [center
            addObserverForName:NSWindowDidEnterFullScreenNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) {
                        SeanceApplyPromotionToWindow(SeanceWindowFromNotification(notification));
                    }];

        SeancePromotionScreenChangedObserver = [center
            addObserverForName:NSWindowDidChangeScreenNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) {
                        SeanceApplyPromotionToWindow(SeanceWindowFromNotification(notification));
                    }];

        SeancePromotionAppLaunchedObserver = [center
            addObserverForName:NSApplicationDidFinishLaunchingNotification
                        object:nil
                         queue:[NSOperationQueue mainQueue]
                    usingBlock:^(NSNotification *notification) {
                        (void)notification;
                        SeanceApplyPromotionToAllWindows();
                    }];

        SeancePromotionInstalled = YES;
        SeanceApplyPromotionToAllWindows();
    };

    if ([NSThread isMainThread]) {
        installBlock();
    } else {
        dispatch_sync(dispatch_get_main_queue(), installBlock);
    }
    return true;
}

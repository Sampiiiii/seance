#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <objc/message.h>
#import <objc/runtime.h>
#include <stdbool.h>

static id seanceSparkleController = nil;

static NSString *SeanceStringFromUtf8(const char *raw) {
    if (raw == NULL) {
        return nil;
    }
    return [NSString stringWithUTF8String:raw];
}

static BOOL SeanceLoadSparkleBundle(void) {
    NSBundle *mainBundle = [NSBundle mainBundle];
    NSString *frameworksPath = [mainBundle privateFrameworksPath];
    if (frameworksPath == nil) {
        return NO;
    }

    NSString *sparklePath = [frameworksPath stringByAppendingPathComponent:@"Sparkle.framework"];
    NSBundle *sparkleBundle = [NSBundle bundleWithPath:sparklePath];
    if (sparkleBundle == nil) {
        return NO;
    }
    if (![sparkleBundle isLoaded] && ![sparkleBundle load]) {
        return NO;
    }
    return NSClassFromString(@"SPUStandardUpdaterController") != Nil;
}

bool seance_sparkle_initialize(const char *feed_url) {
    if (!SeanceLoadSparkleBundle()) {
        return false;
    }

    NSString *nsFeedUrl = SeanceStringFromUtf8(feed_url);
    if (nsFeedUrl != nil) {
        [[NSUserDefaults standardUserDefaults] setObject:nsFeedUrl forKey:@"SUFeedURL"];
    }

    if (seanceSparkleController == nil) {
        Class controllerClass = NSClassFromString(@"SPUStandardUpdaterController");
        if (controllerClass == Nil) {
            return false;
        }

        SEL initSelector = sel_registerName("initWithStartingUpdater:updaterDelegate:userDriverDelegate:");
        id allocated = ((id (*)(id, SEL))objc_msgSend)(controllerClass, sel_registerName("alloc"));
        seanceSparkleController =
            ((id (*)(id, SEL, BOOL, id, id))objc_msgSend)(allocated, initSelector, YES, nil, nil);
        if (seanceSparkleController == nil) {
            return false;
        }
    }

    return true;
}

bool seance_sparkle_check_for_updates(void) {
    if (!seance_sparkle_initialize(NULL)) {
        return false;
    }

    SEL updaterSelector = sel_registerName("updater");
    id updater = ((id (*)(id, SEL))objc_msgSend)(seanceSparkleController, updaterSelector);
    if (updater == nil) {
        return false;
    }

    ((void (*)(id, SEL, id))objc_msgSend)(updater, sel_registerName("checkForUpdates:"), nil);
    return true;
}

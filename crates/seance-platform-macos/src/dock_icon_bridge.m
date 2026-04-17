#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <ImageIO/ImageIO.h>
#import <dispatch/dispatch.h>
#include <stdbool.h>

static const NSTimeInterval SeanceMinimumFrameDelay = 0.08;
static const NSTimeInterval SeanceMaximumFrameDelay = 1.0;
static const NSTimeInterval SeanceFallbackFrameDelay = 0.10;

static NSString *SeanceStringFromUtf8(const char *raw) {
    if (raw == NULL) {
        return nil;
    }
    return [NSString stringWithUTF8String:raw];
}

static NSTimeInterval SeanceClampFrameDelay(NSNumber *delayNumber) {
    double delay = delayNumber.doubleValue;
    if (delay <= 0.0) {
        delay = SeanceFallbackFrameDelay;
    }
    if (delay < SeanceMinimumFrameDelay) {
        return SeanceMinimumFrameDelay;
    }
    if (delay > SeanceMaximumFrameDelay) {
        return SeanceMaximumFrameDelay;
    }
    return delay;
}

@interface SeanceDockIconAnimator : NSObject
@property(nonatomic, copy) NSArray<NSImage *> *frames;
@property(nonatomic, copy) NSArray<NSNumber *> *delays;
@property(nonatomic, strong) NSImageView *imageView;
@property(nonatomic, strong) NSTimer *timer;
@property(nonatomic, assign) NSUInteger frameIndex;
- (BOOL)startWithResourceName:(NSString *)resourceName extension:(NSString *)extension;
@end

@implementation SeanceDockIconAnimator

- (BOOL)startWithResourceName:(NSString *)resourceName extension:(NSString *)extension {
    NSURL *resourceURL = [[NSBundle mainBundle] URLForResource:resourceName withExtension:extension];
    if (resourceURL == nil) {
        return NO;
    }

    CGImageSourceRef source = CGImageSourceCreateWithURL((__bridge CFURLRef)resourceURL, NULL);
    if (source == NULL) {
        return NO;
    }

    NSMutableArray<NSImage *> *frames = [NSMutableArray array];
    NSMutableArray<NSNumber *> *delays = [NSMutableArray array];
    size_t frameCount = CGImageSourceGetCount(source);
    for (size_t index = 0; index < frameCount; index++) {
        CGImageRef cgImage = CGImageSourceCreateImageAtIndex(source, index, NULL);
        if (cgImage == NULL) {
            continue;
        }

        NSDictionary *properties =
            (__bridge_transfer NSDictionary *)CGImageSourceCopyPropertiesAtIndex(source, index, NULL);
        NSDictionary *gifProperties = properties[(NSString *)kCGImagePropertyGIFDictionary];
        NSNumber *delayNumber = gifProperties[(NSString *)kCGImagePropertyGIFUnclampedDelayTime];
        if (delayNumber == nil || delayNumber.doubleValue <= 0.0) {
            delayNumber = gifProperties[(NSString *)kCGImagePropertyGIFDelayTime];
        }

        NSImage *frameImage = [[NSImage alloc] initWithCGImage:cgImage size:NSMakeSize(128.0, 128.0)];
        CGImageRelease(cgImage);

        [frames addObject:frameImage];
        [delays addObject:@(SeanceClampFrameDelay(delayNumber))];
    }
    CFRelease(source);

    if (frames.count == 0 || frames.count != delays.count) {
        return NO;
    }

    self.frames = frames;
    self.delays = delays;
    self.frameIndex = 0;

    if (self.imageView == nil) {
        self.imageView = [[NSImageView alloc] initWithFrame:NSMakeRect(0.0, 0.0, 128.0, 128.0)];
        self.imageView.imageScaling = NSImageScaleProportionallyUpOrDown;
        self.imageView.animates = NO;
    }

    [NSApp.dockTile setContentView:self.imageView];
    [self displayCurrentFrame];
    return YES;
}

- (void)displayCurrentFrame {
    if (self.frames.count == 0) {
        return;
    }

    self.imageView.image = self.frames[self.frameIndex];
    [NSApp.dockTile display];
    [self scheduleNextFrame];
}

- (void)advanceFrame:(NSTimer *)timer {
    (void)timer;
    if (self.frames.count == 0) {
        return;
    }

    self.frameIndex = (self.frameIndex + 1) % self.frames.count;
    [self displayCurrentFrame];
}

- (void)scheduleNextFrame {
    [self.timer invalidate];

    NSTimeInterval delay = [self.delays[self.frameIndex] doubleValue];
    self.timer = [NSTimer timerWithTimeInterval:delay
                                         target:self
                                       selector:@selector(advanceFrame:)
                                       userInfo:nil
                                        repeats:NO];
    [[NSRunLoop mainRunLoop] addTimer:self.timer forMode:NSRunLoopCommonModes];
}

@end

static SeanceDockIconAnimator *seanceDockIconAnimator = nil;

static BOOL SeanceStartDockIconAnimation(NSString *resourceName, NSString *extension) {
    if (resourceName == nil || extension == nil || NSApp == nil) {
        return NO;
    }

    if (seanceDockIconAnimator != nil) {
        return YES;
    }

    SeanceDockIconAnimator *animator = [[SeanceDockIconAnimator alloc] init];
    if (![animator startWithResourceName:resourceName extension:extension]) {
        return NO;
    }

    seanceDockIconAnimator = animator;
    return YES;
}

bool seance_dock_icon_start(const char *resource_name, const char *extension) {
    NSString *resourceName = SeanceStringFromUtf8(resource_name);
    NSString *resourceExtension = SeanceStringFromUtf8(extension);
    if (resourceName == nil || resourceExtension == nil) {
        return false;
    }

    if ([NSThread isMainThread]) {
        return SeanceStartDockIconAnimation(resourceName, resourceExtension);
    }

    __block BOOL started = NO;
    dispatch_sync(dispatch_get_main_queue(), ^{
      started = SeanceStartDockIconAnimation(resourceName, resourceExtension);
    });
    return started;
}

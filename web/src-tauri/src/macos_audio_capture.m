#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreAudio/CoreAudioTypes.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreMedia/CoreMedia.h>
#import <math.h>

typedef void (*VEAudioSamplesCallback)(const int16_t *samples, size_t count, uint32_t sample_rate);
typedef void (*VEAudioEventCallback)(int32_t event, const char *message);

enum {
  VEAudioEventStarted = 1,
  VEAudioEventStopped = 2,
  VEAudioEventError = 3,
};

API_AVAILABLE(macos(13.0))
@interface VEAudioCapture : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, strong) SCStream *stream;
@property(nonatomic, assign) VEAudioSamplesCallback samplesCallback;
@property(nonatomic, assign) VEAudioEventCallback eventCallback;
@property(nonatomic, assign) BOOL stopping;
- (instancetype)initWithSamplesCallback:(VEAudioSamplesCallback)samplesCallback
                           eventCallback:(VEAudioEventCallback)eventCallback;
- (void)start;
- (void)stop;
@end

static id activeCapture;

static void emitEvent(VEAudioEventCallback callback, int32_t event, NSString *message) {
  if (callback == NULL) return;
  callback(event, message == nil ? NULL : message.UTF8String);
}

static float sampleValue(const AudioBuffer *buffer,
                         size_t sampleIndex,
                         const AudioStreamBasicDescription *format) {
  if (buffer == NULL || buffer->mData == NULL) return 0.0f;
  const size_t bytesPerSample = format->mBitsPerChannel / 8;
  if (bytesPerSample == 0 || (sampleIndex + 1) * bytesPerSample > buffer->mDataByteSize) {
    return 0.0f;
  }
  const uint8_t *data = (const uint8_t *)buffer->mData + sampleIndex * bytesPerSample;
  if ((format->mFormatFlags & kAudioFormatFlagIsFloat) != 0) {
    if (format->mBitsPerChannel == 32) return *(const float *)data;
    if (format->mBitsPerChannel == 64) return (float)*(const double *)data;
  }
  if ((format->mFormatFlags & kAudioFormatFlagIsSignedInteger) != 0) {
    if (format->mBitsPerChannel == 16) return *(const int16_t *)data / 32768.0f;
    if (format->mBitsPerChannel == 32) return *(const int32_t *)data / 2147483648.0f;
  }
  return 0.0f;
}

@implementation VEAudioCapture

- (instancetype)initWithSamplesCallback:(VEAudioSamplesCallback)samplesCallback
                           eventCallback:(VEAudioEventCallback)eventCallback {
  self = [super init];
  if (self) {
    _samplesCallback = samplesCallback;
    _eventCallback = eventCallback;
  }
  return self;
}

- (void)start {
  if (@available(macOS 13.0, *)) {
    if (!CGPreflightScreenCaptureAccess()) {
      if (!CGRequestScreenCaptureAccess()) {
        emitEvent(self.eventCallback, VEAudioEventError,
                  @"尚未授予屏幕与系统音频录制权限，请在系统设置 > 隐私与安全性 > 屏幕与系统音频录制中允许 Voice Elf，随后重新打开应用");
        activeCapture = nil;
        return;
      }
    }
    __weak VEAudioCapture *weakSelf = self;
    [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                               onScreenWindowsOnly:NO
                                                 completionHandler:^(SCShareableContent *content,
                                                                     NSError *error) {
      dispatch_async(dispatch_get_main_queue(), ^{
        VEAudioCapture *capture = weakSelf;
        if (capture == nil || capture.stopping) return;
        if (error != nil) {
          emitEvent(capture.eventCallback, VEAudioEventError,
                    @"无法读取可共享屏幕，请在系统设置的隐私与安全性中允许 Voice Elf 录制屏幕和系统音频");
          activeCapture = nil;
          return;
        }
        SCDisplay *selectedDisplay = nil;
        CGDirectDisplayID mainDisplayID = CGMainDisplayID();
        for (SCDisplay *display in content.displays) {
          if (display.displayID == mainDisplayID) {
            selectedDisplay = display;
            break;
          }
        }
        if (selectedDisplay == nil) selectedDisplay = content.displays.firstObject;
        if (selectedDisplay == nil) {
          emitEvent(capture.eventCallback, VEAudioEventError,
                    @"未读取到可录制屏幕，请确认 Voice Elf 已获得屏幕与系统音频录制权限并重新打开应用");
          activeCapture = nil;
          return;
        }

        SCContentFilter *filter = [[SCContentFilter alloc]
            initWithDisplay:selectedDisplay
            excludingApplications:@[]
            exceptingWindows:@[]];
        SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
        configuration.width = 2;
        configuration.height = 2;
        configuration.minimumFrameInterval = CMTimeMake(1, 10);
        configuration.queueDepth = 3;
        configuration.showsCursor = NO;
        configuration.capturesAudio = YES;
        configuration.excludesCurrentProcessAudio = YES;
        configuration.sampleRate = 48000;
        configuration.channelCount = 1;

        capture.stream = [[SCStream alloc] initWithFilter:filter
                                            configuration:configuration
                                                 delegate:capture];
        NSError *outputError = nil;
        dispatch_queue_t audioQueue = dispatch_queue_create(
            "com.voiceelf.client.system-audio", DISPATCH_QUEUE_SERIAL);
        BOOL added = [capture.stream addStreamOutput:capture
                                               type:SCStreamOutputTypeAudio
                                 sampleHandlerQueue:audioQueue
                                              error:&outputError];
        if (!added) {
          emitEvent(capture.eventCallback, VEAudioEventError,
                    outputError.localizedDescription ?: @"无法连接系统音频输出");
          capture.stream = nil;
          activeCapture = nil;
          return;
        }
        [capture.stream startCaptureWithCompletionHandler:^(NSError *startError) {
          if (startError != nil) {
            emitEvent(capture.eventCallback, VEAudioEventError,
                      @"无法启动系统内录，请确认已授予屏幕与系统音频录制权限后重试");
            dispatch_async(dispatch_get_main_queue(), ^{
              capture.stream = nil;
              activeCapture = nil;
            });
          } else {
            emitEvent(capture.eventCallback, VEAudioEventStarted, NULL);
          }
        }];
      });
    }];
  } else {
    emitEvent(self.eventCallback, VEAudioEventError, @"系统内录需要 macOS 13 或更高版本");
    activeCapture = nil;
  }
}

- (void)stop {
  self.stopping = YES;
  SCStream *stream = self.stream;
  if (stream == nil) {
    emitEvent(self.eventCallback, VEAudioEventStopped, NULL);
    activeCapture = nil;
    return;
  }
  VEAudioEventCallback callback = self.eventCallback;
  [stream stopCaptureWithCompletionHandler:^(NSError *error) {
    emitEvent(callback, error == nil ? VEAudioEventStopped : VEAudioEventError,
              error.localizedDescription);
    dispatch_async(dispatch_get_main_queue(), ^{
      ((VEAudioCapture *)activeCapture).stream = nil;
      activeCapture = nil;
    });
  }];
}

- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
                  ofType:(SCStreamOutputType)type API_AVAILABLE(macos(13.0)) {
  if (type != SCStreamOutputTypeAudio || self.stopping || self.samplesCallback == NULL ||
      !CMSampleBufferIsValid(sampleBuffer) || !CMSampleBufferDataIsReady(sampleBuffer)) {
    return;
  }
  CMAudioFormatDescriptionRef description =
      (CMAudioFormatDescriptionRef)CMSampleBufferGetFormatDescription(sampleBuffer);
  const AudioStreamBasicDescription *format =
      CMAudioFormatDescriptionGetStreamBasicDescription(description);
  if (format == NULL || format->mFormatID != kAudioFormatLinearPCM ||
      format->mChannelsPerFrame == 0) {
    return;
  }
  UInt32 channels = format->mChannelsPerFrame;
  size_t listSize = offsetof(AudioBufferList, mBuffers) + sizeof(AudioBuffer) * channels;
  AudioBufferList *list = calloc(1, listSize);
  if (list == NULL) return;
  CMBlockBufferRef blockBuffer = NULL;
  OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
      sampleBuffer, NULL, list, listSize, NULL, NULL,
      kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, &blockBuffer);
  if (status != noErr) {
    free(list);
    if (blockBuffer != NULL) CFRelease(blockBuffer);
    return;
  }
  if (list->mNumberBuffers == 0) {
    free(list);
    if (blockBuffer != NULL) CFRelease(blockBuffer);
    return;
  }

  CMItemCount frameCount = CMSampleBufferGetNumSamples(sampleBuffer);
  int16_t *samples = calloc((size_t)frameCount, sizeof(int16_t));
  if (samples != NULL) {
    BOOL nonInterleaved = (format->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
    for (CMItemCount frame = 0; frame < frameCount; frame++) {
      float mixed = 0.0f;
      for (UInt32 channel = 0; channel < channels; channel++) {
        UInt32 bufferIndex = nonInterleaved ? MIN(channel, list->mNumberBuffers - 1) : 0;
        size_t sampleIndex = nonInterleaved ? (size_t)frame : (size_t)frame * channels + channel;
        mixed += sampleValue(&list->mBuffers[bufferIndex], sampleIndex, format) / channels;
      }
      mixed = fmaxf(-1.0f, fminf(1.0f, mixed));
      samples[frame] = (int16_t)lrintf(mixed * 32767.0f);
    }
    self.samplesCallback(samples, (size_t)frameCount, (uint32_t)format->mSampleRate);
    free(samples);
  }
  free(list);
  if (blockBuffer != NULL) CFRelease(blockBuffer);
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
  if (!self.stopping) emitEvent(self.eventCallback, VEAudioEventError, error.localizedDescription);
  dispatch_async(dispatch_get_main_queue(), ^{
    ((VEAudioCapture *)activeCapture).stream = nil;
    activeCapture = nil;
  });
}

@end

bool voice_elf_macos_audio_supported(void) {
  if (@available(macOS 13.0, *)) return true;
  return false;
}

void voice_elf_macos_audio_start(VEAudioSamplesCallback samplesCallback,
                                 VEAudioEventCallback eventCallback) {
  if (@available(macOS 13.0, *)) {
    dispatch_async(dispatch_get_main_queue(), ^{
      if (activeCapture != nil) {
        emitEvent(eventCallback, VEAudioEventError, @"系统内录已在运行");
        return;
      }
      activeCapture = [[VEAudioCapture alloc] initWithSamplesCallback:samplesCallback
                                                        eventCallback:eventCallback];
      [activeCapture start];
    });
  } else {
    emitEvent(eventCallback, VEAudioEventError, @"系统内录需要 macOS 13 或更高版本");
  }
}

void voice_elf_macos_audio_stop(void) {
  if (@available(macOS 13.0, *)) {
    dispatch_async(dispatch_get_main_queue(), ^{
      [(VEAudioCapture *)activeCapture stop];
    });
  }
}

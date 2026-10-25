// Public WebKit APIs only. Host pulls bounded layout measurements; no document messages or file grants.
#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <QuartzCore/QuartzCore.h>

@interface GroveHTMLClip : NSView
@property(nonatomic, strong) NSUndoManager *editingUndoManager;
@property(nonatomic) NSRect exclusion;
@property(nonatomic) BOOL passivePointer;
@end
@implementation GroveHTMLClip
- (BOOL)isFlipped { return YES; }
- (NSView *)hitTest:(NSPoint)point {
    if (self.passivePointer) return nil;
    return [super hitTest:point];
}
- (NSUndoManager *)undoManager {
    if (!_editingUndoManager) _editingUndoManager = [NSUndoManager new];
    return _editingUndoManager;
}
@end

// An integrated report shares the conversation's scroll: wheel input goes to
// the GPUI host view, which hit-tests it into the enclosing scroll container.
@interface GroveReportWebView : WKWebView
@property(nonatomic) BOOL integrated;
@property(nonatomic, weak) NSView *host;
@end
@implementation GroveReportWebView
- (void)scrollWheel:(NSEvent *)event {
    NSView *host = self.host;
    if (self.integrated && host) [host scrollWheel:event];
    else [super scrollWheel:event];
}
@end

@interface GroveHTMLDocument : NSObject <WKNavigationDelegate, WKUIDelegate>
@property(nonatomic, strong) GroveHTMLClip *clip;
@property(nonatomic, strong) WKWebView *web;
@property(nonatomic) double contentHeight;
@property(nonatomic) BOOL measuring;
@property(nonatomic) int status; // 0 loading, 1 ready, 2 failed
@property(nonatomic) BOOL initialNavigation;
@property(nonatomic) BOOL disposed;
@property(nonatomic) uint64_t paintGeneration;
@end

@implementation GroveHTMLDocument
- (void)webView:(WKWebView *)webView decidePolicyForNavigationAction:(WKNavigationAction *)action decisionHandler:(void (^)(WKNavigationActionPolicy))handler {
    // Only the one host-issued loadHTMLString load. All later navigations,
    // including forms, downloads, custom protocols and child documents, stop here.
    BOOL initial = self.initialNavigation && action.targetFrame.isMainFrame &&
        [action.request.URL.absoluteString isEqualToString:@"grove-html://document/"] &&
        action.navigationType == WKNavigationTypeOther;
    if (initial) self.initialNavigation = NO;
    NSURLComponents *requested = [NSURLComponents componentsWithURL:action.request.URL resolvingAgainstBaseURL:NO];
    NSURLComponents *current = [NSURLComponents componentsWithURL:webView.URL resolvingAgainstBaseURL:NO];
    BOOL changesFragment = (requested.fragment || current.fragment) && ![requested.fragment isEqual:current.fragment];
    requested.fragment = nil; current.fragment = nil;
    BOOL sameDocument = action.targetFrame.isMainFrame && changesFragment && [requested.URL isEqual:current.URL];
#ifdef GROVE_HTML_NATIVE_TEST
    fprintf(stderr, "navigation request=%s current=%s main=%d initial=%d sameDocument=%d\n", action.request.URL.absoluteString.UTF8String, webView.URL.absoluteString.UTF8String, action.targetFrame.isMainFrame, initial, sameDocument);
#endif
    handler(initial || sameDocument ? WKNavigationActionPolicyAllow : WKNavigationActionPolicyCancel);
}
- (void)webView:(WKWebView *)webView didFinishNavigation:(WKNavigation *)navigation {
    if (!self.disposed) self.status = 1;
}
- (void)webView:(WKWebView *)webView didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    if (!self.disposed && error.code != NSURLErrorCancelled) self.status = 2;
}
- (void)webView:(WKWebView *)webView didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    if (!self.disposed && error.code != NSURLErrorCancelled) self.status = 2;
}
- (void)webViewWebContentProcessDidTerminate:(WKWebView *)webView {
    if (!self.disposed) self.status = 2;
}
- (void)webView:(WKWebView *)webView printFrame:(WKFrameInfo *)frame {}
- (WKWebView *)webView:(WKWebView *)webView createWebViewWithConfiguration:(WKWebViewConfiguration *)configuration forNavigationAction:(WKNavigationAction *)action windowFeatures:(WKWindowFeatures *)features { return nil; }
- (void)webView:(WKWebView *)webView runOpenPanelWithParameters:(WKOpenPanelParameters *)parameters initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSArray<NSURL *> *))handler { handler(nil); }
- (void)webView:(WKWebView *)webView runJavaScriptAlertPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(void))handler { handler(); }
- (void)webView:(WKWebView *)webView runJavaScriptConfirmPanelWithMessage:(NSString *)message initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(BOOL))handler { handler(NO); }
- (void)webView:(WKWebView *)webView runJavaScriptTextInputPanelWithPrompt:(NSString *)prompt defaultText:(NSString *)text initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSString *))handler { handler(nil); }
- (void)webView:(WKWebView *)webView requestMediaCapturePermissionForOrigin:(WKSecurityOrigin *)origin initiatedByFrame:(WKFrameInfo *)frame type:(WKMediaCaptureType)type decisionHandler:(void (^)(WKPermissionDecision))handler API_AVAILABLE(macos(12.0)) { handler(WKPermissionDecisionDeny); }
@end

static NSHashTable<GroveHTMLDocument *> *grove_html_documents(void) {
    static NSHashTable<GroveHTMLDocument *> *documents;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ documents = [NSHashTable weakObjectsHashTable]; });
    return documents;
}
#include "inline_html_keyboard.inc"
#include "inline_html_accessibility.inc"
void grove_html_focus_host(void *parentPointer) {
    NSView *parent = (__bridge NSView *)parentPointer;
    NSResponder *responder = parent.window.firstResponder;
    if (![responder isKindOfClass:NSView.class]) return;
    for (GroveHTMLDocument *document in grove_html_documents()) {
        if (document.disposed || document.clip.superview != parent ||
            document.clip.hiddenOrHasHiddenAncestor ||
            ![(NSView *)responder isDescendantOf:document.clip]) continue;
        [parent.window makeFirstResponder:parent];
        return;
    }
}
void grove_html_hide(void *handle);
void grove_html_hide_all(void) {
    for (GroveHTMLDocument *document in grove_html_documents()) grove_html_hide((__bridge void *)document);
}

void *grove_html_create(const char *bytes, size_t length) {
    NSCAssert([NSThread isMainThread], @"HTML views require main thread");
    NSString *html = [[NSString alloc] initWithBytes:bytes length:length encoding:NSUTF8StringEncoding];
    if (!html) return NULL;
    GroveHTMLDocument *document = [GroveHTMLDocument new];
    [grove_html_documents() addObject:document];
    grove_html_install_edit_monitor();
    document.clip = [GroveHTMLClip new];
    document.clip.wantsLayer = YES;
    document.clip.layer.masksToBounds = YES;
    document.clip.hidden = YES;
    WKWebViewConfiguration *config = [WKWebViewConfiguration new];
    config.websiteDataStore = [WKWebsiteDataStore nonPersistentDataStore];
    config.preferences.javaScriptCanOpenWindowsAutomatically = NO;
    if (@available(macOS 12.3, *)) config.preferences.elementFullscreenEnabled = NO;
    config.mediaTypesRequiringUserActionForPlayback = WKAudiovisualMediaTypeAll;
    // CSP and the rule list cover URL-loading APIs. WebRTC can send STUN/UDP
    // outside that loader; remove its constructors before author JS, in every
    // frame. Nonconfigurable own properties cannot be restored by author JS.
    NSString *guard = @"for(const name of ['RTCPeerConnection','webkitRTCPeerConnection','RTCIceGatherer','WebTransport','Worker','SharedWorker','ServiceWorker','Notification']){try{Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});}catch{}}";
    [config.userContentController addUserScript:[[WKUserScript alloc] initWithSource:guard injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:NO]];
    document.web = [[GroveReportWebView alloc] initWithFrame:NSZeroRect configuration:config];
    document.web.navigationDelegate = document;
    document.web.UIDelegate = document;
    document.web.allowsLinkPreview = NO;
    [document.clip addSubview:document.web];
    // Fail closed: no HTML is loaded until WebKit has compiled and installed
    // this deny-all network/file/blob/custom-protocol content rule list.
    NSString *rules = @"[{\"trigger\":{\"url-filter\":\".*\"},\"action\":{\"type\":\"block\"}},{\"trigger\":{\"url-filter\":\"^grove-html://document/\"},\"action\":{\"type\":\"ignore-previous-rules\"}},{\"trigger\":{\"url-filter\":\"^about:blank\"},\"action\":{\"type\":\"ignore-previous-rules\"}},{\"trigger\":{\"url-filter\":\"^data:\"},\"action\":{\"type\":\"ignore-previous-rules\"}}]";
    __weak GroveHTMLDocument *weak = document;
    [[WKContentRuleListStore defaultStore] compileContentRuleListForIdentifier:@"grove-self-contained-html-v1" encodedContentRuleList:rules completionHandler:^(WKContentRuleList *list, NSError *error) {
        GroveHTMLDocument *doc = weak;
        if (!doc || doc.disposed) return;
        if (!list || error) { doc.status = 2; return; }
        [doc.web.configuration.userContentController addContentRuleList:list];
        doc.initialNavigation = YES;
        [doc.web loadHTMLString:html baseURL:[NSURL URLWithString:@"grove-html://document/"]];
    }];
    return (__bridge_retained void *)document;
}
// A pull-only, read-only measurement. No arbitrary document payload reaches Rust.
// Measure body geometry rather than scrollHeight (which is at least viewport height).
double grove_html_report_height(void *handle) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    if (!doc.disposed && doc.status == 1 && !doc.measuring) {
        doc.measuring = YES;
        __weak GroveHTMLDocument *weak = doc;
        [doc.web evaluateJavaScript:@"Math.ceil(document.body ? document.body.getBoundingClientRect().height : 0)" completionHandler:^(id value, NSError *error) {
            GroveHTMLDocument *current = weak;
            if (!current || current.disposed) return;
            current.measuring = NO;
            double height = [value isKindOfClass:NSNumber.class] ? [value doubleValue] : 0;
            if (!error && isfinite(height) && height > 0) current.contentHeight = height;
        }];
    }
    return doc.contentHeight;
}
void grove_html_report_theme(void *handle, const char *bytes, size_t length, bool integrated) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    ((GroveReportWebView *)doc.web).integrated = integrated;
    NSString *script = [[NSString alloc] initWithBytes:bytes length:length encoding:NSUTF8StringEncoding];
    if (script && !doc.disposed) [doc.web evaluateJavaScript:script completionHandler:nil];
}
int grove_html_status(void *handle) { return ((__bridge GroveHTMLDocument *)handle).status; }
void grove_html_hide(void *handle) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    NSView *parent = doc.clip.superview;
    NSWindow *window = doc.clip.window;
    NSResponder *responder = window.firstResponder;
    if ([responder isKindOfClass:NSView.class] && [(NSView *)responder isDescendantOf:doc.clip]) {
        [window makeFirstResponder:doc.clip.superview];
    }
    doc.clip.hidden = YES;
    [doc.clip removeFromSuperview];
    if (parent) NSAccessibilityPostNotification(parent.window.contentView, NSAccessibilityLayoutChangedNotification);
}
// Persistent GPUI element state retires when a later frame omits the surface.
// Defer retirement so an inline/expanded handoff can repaint before detaching.
void grove_html_hide_after_paint(void *handle) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    uint64_t generation = doc.paintGeneration;
    __weak GroveHTMLDocument *weak = doc;
    dispatch_async(dispatch_get_main_queue(), ^{
        GroveHTMLDocument *current = weak;
        if (current && !current.disposed && current.paintGeneration == generation) grove_html_hide((__bridge void *)current);
    });
}
void grove_html_exclude(void *handle, double x, double y, double width, double height) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    if (doc.disposed) return;
    doc.clip.passivePointer = width > 0 && height > 0;
    NSRect frame = doc.clip.frame;
    // Rust coordinates are top-left logical window points, as is this flipped clip.
    NSView *parent = doc.clip.superview;
    CGFloat top = parent.isFlipped ? frame.origin.y : parent.bounds.size.height - NSMaxY(frame);
    NSRect hole = NSIntersectionRect(doc.clip.bounds, NSMakeRect(x-frame.origin.x, y-top, width, height));
    doc.clip.exclusion = hole;
    if (NSIsEmptyRect(hole)) { doc.clip.layer.mask = nil; return; }
    CAShapeLayer *mask = [CAShapeLayer layer];
    CGMutablePathRef path = CGPathCreateMutable();
    CGPathAddRect(path, NULL, NSRectToCGRect(doc.clip.bounds));
    CGPathAddRect(path, NULL, NSRectToCGRect(hole));
    mask.path = path;
    mask.fillRule = kCAFillRuleEvenOdd;
    doc.clip.layer.mask = mask;
    CGPathRelease(path);
}

void grove_html_show(void *handle, void *parentPointer, double x, double y, double width, double height, double clipX, double clipY, double clipWidth, double clipHeight) {
    GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
    NSView *parent = (__bridge NSView *)parentPointer;
    if (doc.disposed || !parent.window || width <= 0 || height <= 0 || clipWidth <= 0 || clipHeight <= 0) { grove_html_hide(handle); return; }
    doc.paintGeneration += 1;
    grove_html_ax_install(parent.window.contentView);
    BOOL attaching = doc.clip.superview != parent || doc.clip.hidden;
    [parent addSubview:doc.clip positioned:NSWindowAbove relativeTo:nil];
    CGFloat nativeY = parent.isFlipped ? clipY : parent.bounds.size.height - clipY - clipHeight;
    doc.clip.frame = NSMakeRect(clipX, nativeY, clipWidth, clipHeight);
    doc.web.frame = NSMakeRect(x - clipX, y - clipY, width, height);
    ((GroveReportWebView *)doc.web).host = parent;
    doc.clip.hidden = NO;
    if (attaching) NSAccessibilityPostNotification(parent.window.contentView, NSAccessibilityLayoutChangedNotification);
}
void grove_html_destroy(void *handle) {
    if (!handle) return;
    GroveHTMLDocument *doc = (__bridge_transfer GroveHTMLDocument *)handle;
    grove_html_hide((__bridge void *)doc);
    doc.disposed = YES;
    [grove_html_documents() removeObject:doc];
    grove_html_remove_edit_monitor_if_unused();
    [doc.web stopLoading];
    doc.web.navigationDelegate = nil;
    doc.web.UIDelegate = nil;
    [doc.clip removeFromSuperview];
}

// Explicit user action only: resolve the HTTPS handler, not the HTML file's
// association (which may be an editor). The dummy URL is never opened/requested.
static NSURL *grove_html_browser_application(void) {
    return [NSWorkspace.sharedWorkspace URLForApplicationToOpenURL:[NSURL URLWithString:@"https://example.invalid/"]];
}
typedef void (*GroveHTMLBrowserCompletion)(void *, const char *, size_t);
void grove_html_open_browser(const char *bytes, size_t length, void *context, GroveHTMLBrowserCompletion completion) {
    NSString *string = [[NSString alloc] initWithBytes:bytes length:length encoding:NSUTF8StringEncoding];
    NSURL *url = string ? [NSURL URLWithString:string] : nil;
    NSURL *application = grove_html_browser_application();
    if (!url.isFileURL || !application) {
        const char *message = "Couldn’t resolve the default web browser for this HTML file";
        completion(context, message, strlen(message));
        return;
    }
    NSWorkspaceOpenConfiguration *config = [NSWorkspaceOpenConfiguration configuration];
    [NSWorkspace.sharedWorkspace openURLs:@[url] withApplicationAtURL:application configuration:config completionHandler:^(NSRunningApplication *app, NSError *error) {
        (void)app;
        const char *message = error ? error.localizedDescription.UTF8String : NULL;
        completion(context, message, message ? strlen(message) : 0);
    }];
}

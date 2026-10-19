// Standalone native boundary/state test. Compile with clang -fobjc-arc -fblocks
// -framework AppKit -framework WebKit -framework QuartzCore. No Grove app launch.
#define GROVE_HTML_NATIVE_TEST 1
#import "../src/platform/inline_html.m"

static NSString *probePort = @"19451";
static id evaluate(WKWebView *web, NSString *script) {
    __block BOOL done = NO;
    __block id value;
    __block NSError *failure;
    script = [script stringByReplacingOccurrencesOfString:@"19451" withString:probePort];
    [web evaluateJavaScript:script completionHandler:^(id result, NSError *error) { value = result; failure = error; done = YES; }];
    NSDate *limit = [NSDate dateWithTimeIntervalSinceNow:5];
    while (!done && limit.timeIntervalSinceNow > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    return failure ? @{ @"error": failure.localizedDescription } : value ?: NSNull.null;
}
static void pump(double seconds) {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (until.timeIntervalSinceNow > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
}
#include "inline-html-keyboard.inc"
#include "inline-html-accessibility.inc"
int main(int argc, const char **argv) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        if (argc != 2 && argc != 3) { fprintf(stderr, "usage: verify-inline-html-webkit HTML\n"); return 2; }
        if (argc == 3) probePort = @(argv[2]);
        NSData *source = [NSData dataWithContentsOfFile:@(argv[1])];
        void *handle = grove_html_create(source.bytes, source.length);
        if (!handle) return 2;
        GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 640, 480) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 640, 480, 0, 0, 640, 480);
        for (int i = 0; i < 1000 && grove_html_status(handle) == 0; i++) pump(0.01);
        NSMutableDictionary *results = [NSMutableDictionary new];
        results[@"loaded"] = @(grove_html_status(handle) == 1);
        results[@"accessibility"] = grove_html_verify_accessibility(doc);
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 640, 480, 0, 0, 640, 480);
        grove_html_keyboard_probes(doc, window, results);
        results[@"dynamic_js"] = evaluate(doc.web, @"new Function('return 6*7')()===42 && eval('21*2')===42");
        results[@"initial"] = evaluate(doc.web, @"JSON.stringify({title:document.title,doctype:document.compatMode,guard:typeof RTCPeerConnection,bridge:Object.keys(window.webkit?.messageHandlers||{})})");
        results[@"mutate"] = evaluate(doc.web, @"document.querySelector('#name').value='Retained edit';document.querySelector('#count').click();document.querySelector('#tasks').click();scrollTo(0,700);JSON.stringify({value:document.querySelector('#name').value,count:document.querySelector('#count').textContent,tab:document.body.dataset.tab,scroll:scrollY})");
        grove_html_hide(handle);
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 1000, 800, 20, 20, 600, 400);
        pump(0.1);
        [window makeFirstResponder:doc.web];
        NSResponder *responderBefore = window.firstResponder;
        grove_html_hide_after_paint(handle);
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 1000, 800, 20, 20, 600, 400);
        pump(0.05);
        results[@"replacement_frame_keeps_surface"] = @((BOOL)(doc.clip.superview == window.contentView && !doc.clip.hidden && window.firstResponder == responderBefore));
        grove_html_hide_after_paint(handle);
        pump(0.05);
        results[@"omitted_frame_retires_surface"] = @((BOOL)(doc.clip.hidden && doc.clip.superview == nil));
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 1000, 800, 20, 20, 600, 400);
        results[@"retained"] = evaluate(doc.web, @"JSON.stringify({value:document.querySelector('#name').value,count:document.querySelector('#count').textContent,tab:document.body.dataset.tab,scroll:scrollY})");
        results[@"fresh_realm"] = evaluate(doc.web, @"(()=>{let f=document.createElement('iframe');document.body.append(f);let result={window:!!f.contentWindow};try{result.rtc=typeof f.contentWindow.RTCPeerConnection;result.worker=typeof f.contentWindow.Worker;if(f.contentWindow.RTCPeerConnection){let pc=new f.contentWindow.RTCPeerConnection({iceServers:[{urls:'stun:127.0.0.1:19451'}]});pc.createDataChannel('probe');pc.createOffer().then(x=>pc.setLocalDescription(x));result.created=true;}}catch(e){result.error=e.name;}f.remove();return JSON.stringify(result)})()");
        results[@"network"] = evaluate(doc.web, @"(()=>{window.probes={};fetch('http://127.0.0.1:19451/fetch').then(()=>probes.fetch='unexpected',()=>probes.fetch='blocked');try{new WebSocket('ws://127.0.0.1:19451/socket');probes.socket='constructed'}catch{probes.socket='blocked'};for(const [tag,attribute,path] of [['img','src','image'],['script','src','script'],['link','href','style'],['iframe','src','frame']]){let e=document.createElement(tag);e.rel='stylesheet';e[attribute]='http://127.0.0.1:19451/'+path;document.body.append(e)}return 'attempted'})()");
        evaluate(doc.web, @"fetch('file:///etc/hosts').then(()=>window.fileProbe='unexpected',()=>window.fileProbe='blocked')");
        pump(0.2);
        results[@"local_file"] = evaluate(doc.web, @"window.fileProbe");
        results[@"network_results"] = evaluate(doc.web, @"JSON.stringify(window.probes)");
        results[@"set_fragment"] = evaluate(doc.web, @"location.hash='internal-anchor'"); pump(0.2);
        results[@"fragment"] = evaluate(doc.web, @"location.hash");
        results[@"popup_blocked"] = evaluate(doc.web, @"window.open('http://127.0.0.1:19451/popup')===null");
        evaluate(doc.web, @"let f=document.createElement('form');f.action='http://127.0.0.1:19451/form';document.body.append(f);f.submit();location.href='http://127.0.0.1:19451/navigation'");
        pump(1);
        results[@"after_navigation"] = evaluate(doc.web, @"JSON.stringify({value:document.querySelector('#name')?.value,url:location.href})");
        results[@"clipped_frame"] = NSStringFromRect(doc.clip.frame);
        results[@"document_frame"] = NSStringFromRect(doc.web.frame);
        grove_html_hide(handle);
        void *otherHandle = grove_html_create(source.bytes, source.length);
        GroveHTMLDocument *other = (__bridge GroveHTMLDocument *)otherHandle;
        grove_html_show(otherHandle, (__bridge void *)window.contentView, 0, 0, 640, 480, 0, 0, 640, 480);
        grove_html_hide_all();
        grove_html_show(handle, (__bridge void *)window.contentView, 0, 0, 640, 480, 0, 0, 640, 480);
        results[@"expanded_sibling_hidden"] = @((BOOL)(other.clip.hidden && other.clip.superview == nil));
        results[@"expanded_surface_on_top"] = @((BOOL)(window.contentView.subviews.lastObject == doc.clip));
        grove_html_destroy(otherHandle);
        other = nil;
        results[@"browser_application"] = grove_html_browser_application().path ?: @"";
        grove_html_hide(handle);
        results[@"hidden"] = @(doc.clip.hidden);
        grove_html_destroy(handle);
        results[@"detached"] = @(doc.clip.superview == nil && doc.disposed);
        doc = nil;
        pump(0.1);
        NSData *json = [NSJSONSerialization dataWithJSONObject:results options:NSJSONWritingPrettyPrinted error:nil];
        puts([[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding].UTF8String);
        return [results[@"loaded"] boolValue] ? 0 : 1;
    }
}

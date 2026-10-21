// Exploratory macOS POC check; uses the actual native document, no provider mocks.
#import "../src/platform/inline_html.m"
static void pump(double seconds) {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (until.timeIntervalSinceNow > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
}
static double height(void *handle) {
    grove_html_report_height(handle); pump(0.2);
    return grove_html_report_height(handle);
}
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 2) return 2;
        [NSApplication sharedApplication];
        NSData *source = [NSData dataWithContentsOfFile:@(argv[1])];
        void *handle = grove_html_create(source.bytes, source.length);
        if (!handle) return 2;
        GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,800,600) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        grove_html_show(handle, (__bridge void *)window.contentView, 0,0,800,600,0,0,800,600);
        for (int i=0;i<500 && grove_html_status(handle)==0;i++) pump(0.01);
        // Native measurement independent of viewport height, including shrink.
        const char *script = "document.body.style.cssText='margin:0;height:420px';document.body.dataset.retained='yes'";
        grove_html_report_theme(handle, script, strlen(script), true); pump(0.2);
        double initial = height(handle);
        script = "document.body.style.height='180px'";
        grove_html_report_theme(handle, script, strlen(script), true); pump(0.2);
        double shorter = height(handle);
        script = "document.body.style.height='1200px'";
        grove_html_report_theme(handle, script, strlen(script), true); pump(0.2);
        double taller = height(handle);
        __block BOOL retained = NO;
        [doc.web evaluateJavaScript:@"document.body.dataset.retained==='yes' && Object.keys(window.webkit?.messageHandlers||{}).length===0" completionHandler:^(id value,NSError *error){ retained = !error && [value boolValue]; }]; pump(0.2);
        BOOL pass = initial == 420 && shorter == 180 && taller == 1200 && retained;
        printf("{\"initial\":%.0f,\"shorter\":%.0f,\"taller\":%.0f,\"retained_without_messages\":%s,\"pass\":%s}\n",initial,shorter,taller,retained?"true":"false",pass?"true":"false");
        grove_html_destroy(handle);
        return pass?0:1;
    }
}

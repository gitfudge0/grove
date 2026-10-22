// Real WebKit boundary; production JS is extracted by the companion Python script.
#import "../src/platform/inline_html.m"
static void pump(double seconds) {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (until.timeIntervalSinceNow > 0) [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
}
static id evaluate(GroveHTMLDocument *doc, NSString *script) {
    __block id result = nil; __block BOOL done = NO;
    [doc.web evaluateJavaScript:script completionHandler:^(id value, NSError *error) { result = error ? @{ @"error": error.description } : value; done = YES; }];
    for (int i=0;i<500 && !done;i++) pump(0.01);
    return result;
}
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 4) return 2;
        [NSApplication sharedApplication];
        NSData *source = [NSData dataWithContentsOfFile:@(argv[1])];
        void *handle = grove_html_create(source.bytes, source.length);
        if (!handle) return 2;
        GroveHTMLDocument *doc = (__bridge GroveHTMLDocument *)handle;
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,800,600) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        grove_html_show(handle, (__bridge void *)window.contentView, 0,0,800,600,0,0,800,600);
        for (int i=0;i<500 && grove_html_status(handle)==0;i++) pump(0.01);
        for (int i=0;i<500 && ![evaluate(doc,@"window.loadedFonts===true") boolValue];i++) pump(0.01);
        id initial = evaluate(doc,@"({initial_motion:initialMotion,combined_query:query.matches,no_preference:!matchMedia('(PREFERS-REDUCED-MOTION: NO-PREFERENCE)').matches,animation_stopped:getComputedStyle(document.getElementById('animated')).animationName==='none',font_loaded:loadedFonts})");
        NSData *off = [NSData dataWithContentsOfFile:@(argv[2])];
        grove_html_report_theme(handle, off.bytes, off.length, false);pump(0.1);
        id disabled = evaluate(doc,@"document.documentElement.dataset.groveReduceMotion==='false' && document.getElementById('grove-motion').textContent==='' && document.getElementById('retained').value==='keep me'");
        NSData *on = [NSData dataWithContentsOfFile:@(argv[3])];
        grove_html_report_theme(handle, on.bytes, on.length, false);pump(0.1);
        id enabled = evaluate(doc,@"query.matches && document.documentElement.dataset.groveReduceMotion==='true' && getComputedStyle(document.getElementById('animated')).animationName==='none' && document.getElementById('retained').value==='keep me'");
        BOOL pass = [initial isKindOfClass:[NSDictionary class]] && [disabled boolValue] && [enabled boolValue];
        for (NSString *key in @[@"initial_motion",@"combined_query",@"no_preference",@"animation_stopped",@"font_loaded"]) pass = pass && [initial[key] boolValue];
        NSDictionary *result = @{ @"initial": initial?:[NSNull null], @"disabled_without_reload": disabled?:[NSNull null], @"reenabled_without_reload": enabled?:[NSNull null], @"media_change_events": evaluate(doc,@"events")?:[NSNull null], @"pass":@(pass)};
        NSData *json = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted error:nil];
        puts([[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding].UTF8String);
        grove_html_destroy(handle);
        return pass?0:1;
    }
}

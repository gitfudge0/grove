// Native regression for the exact synthetic-parent boundary used by AccessKit.
// Included after evaluate()/pump() in verify-inline-html-webkit.m.
@interface GroveHTMLSyntheticAXParent : NSView
@property(nonatomic, strong) NSAccessibilityElement *syntheticChild;
@end
@implementation GroveHTMLSyntheticAXParent
- (NSArray *)accessibilityChildren { return @[self.syntheticChild]; }
- (id)accessibilityFocusedUIElement { return self.syntheticChild; }
- (id)accessibilityHitTest:(NSPoint)point { return self.syntheticChild; }
@end

static BOOL grove_html_ax_has_role(id element, NSString *role, NSUInteger depth) {
    if (!element || depth > 16) return NO;
    if (![element respondsToSelector:@selector(accessibilityRole)]) return NO;
    if ([[element accessibilityRole] isEqual:role]) return YES;
    if (![element respondsToSelector:@selector(accessibilityChildren)]) return NO;
    NSArray *children = [element accessibilityChildren];
    for (id child in children) {
        if (grove_html_ax_has_role(child, role, depth + 1)) return YES;
    }
    return NO;
}

static NSDictionary *grove_html_verify_accessibility(GroveHTMLDocument *document) {
    NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 640, 480)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    GroveHTMLSyntheticAXParent *parent = [[GroveHTMLSyntheticAXParent alloc] initWithFrame:NSMakeRect(0, 0, 640, 480)];
    parent.syntheticChild = [NSAccessibilityElement accessibilityElementWithRole:NSAccessibilityButtonRole
        frame:NSMakeRect(0, 0, 20, 20) label:@"Synthetic host control" parent:parent];
    window.contentView = parent;
    NSView *renderView = [[NSView alloc] initWithFrame:NSMakeRect(15, 20, 600, 420)];
    [parent addSubview:renderView];
    grove_html_show((__bridge void *)document, (__bridge void *)renderView, 0, 0, 640, 480, 40, 40, 400, 300);
    NSMutableDictionary *checks = [NSMutableDictionary new];
    BOOL webArea = NO;
    // WebKit's remote AX tree activates asynchronously when first queried.
    for (NSUInteger attempt = 0; attempt < 100 && !webArea; attempt++) {
        webArea = grove_html_ax_has_role(parent, @"AXWebArea", 0);
        if (!webArea) pump(0.02);
    }
    // Remote WebKit AX descendants may be opaque in-process. The native root
    // check is valid here; external CUA must establish AXWebArea visibility.
    checks[@"web_area_in_process"] = @(webArea);
    checks[@"native_root_children"] = @((BOOL)([parent.accessibilityChildren containsObject:document.web] &&
        document.web.accessibilityChildren.count > 0));
    checks[@"synthetic_child_preserved"] = @((BOOL)[parent.accessibilityChildren containsObject:parent.syntheticChild]);
    [window makeFirstResponder:parent];
    checks[@"host_focus_preserved"] = @((BOOL)(parent.accessibilityFocusedUIElement == parent.syntheticChild));
    NSPoint outside = [window convertPointToScreen:[parent convertPoint:NSMakePoint(5, 5) toView:nil]];
    checks[@"outside_clip_host_hit"] = @((BOOL)([parent accessibilityHitTest:outside] == parent.syntheticChild));
    [window makeFirstResponder:document.web];
    evaluate(document.web, @"document.querySelector('#name').focus()");
    pump(0.1);
    id focused = parent.accessibilityFocusedUIElement;
    checks[@"native_focus"] = @((BOOL)(focused && focused != parent.syntheticChild));
    NSPoint inside = [window convertPointToScreen:[document.clip convertPoint:NSMakePoint(50, 50) toView:nil]];
    id hit = [parent accessibilityHitTest:inside];
    checks[@"native_hit"] = @((BOOL)(hit && hit != parent.syntheticChild));
    grove_html_hide((__bridge void *)document);
    checks[@"hidden_excluded"] = @((BOOL)([parent.accessibilityChildren isEqual:@[parent.syntheticChild]] &&
        parent.accessibilityFocusedUIElement == parent.syntheticChild &&
        [parent accessibilityHitTest:inside] == parent.syntheticChild));
    grove_html_show((__bridge void *)document, (__bridge void *)renderView, 0, 0, 640, 480, 0, 0, 640, 480);
    document.disposed = YES;
    checks[@"disposed_excluded"] = @((BOOL)([parent.accessibilityChildren isEqual:@[parent.syntheticChild]] &&
        parent.accessibilityFocusedUIElement == parent.syntheticChild &&
        [parent accessibilityHitTest:inside] == parent.syntheticChild));
    document.disposed = NO;
    grove_html_hide((__bridge void *)document);
    return checks;
}

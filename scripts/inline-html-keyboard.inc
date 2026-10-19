// Included after evaluate()/pump() in the standalone native harness. Invoke
// grove_html_keyboard_probes(doc, window, results) once the page is loaded.
@interface GroveHTMLEditProbe : NSView
@property(nonatomic, copy) NSString *lastAction;
@end
@implementation GroveHTMLEditProbe
- (BOOL)acceptsFirstResponder { return YES; }
- (void)selectAll:(id)sender { self.lastAction = @"selectAll:"; }
- (void)copy:(id)sender { self.lastAction = @"copy:"; }
- (void)paste:(id)sender { self.lastAction = @"paste:"; }
- (void)cut:(id)sender { self.lastAction = @"cut:"; }
- (void)undo:(id)sender { self.lastAction = @"undo:"; }
- (void)redo:(id)sender { self.lastAction = @"redo:"; }
@end
static NSEvent *grove_html_edit_test_event(NSWindow *window, NSString *key, NSEventModifierFlags modifiers) {
    return [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint
        modifierFlags:modifiers timestamp:0 windowNumber:window.windowNumber
        context:nil characters:key charactersIgnoringModifiers:key isARepeat:NO keyCode:0];
}
static void grove_html_keyboard_probes(GroveHTMLDocument *doc, NSWindow *window, NSMutableDictionary *results) {
    GroveHTMLEditProbe *probe = [GroveHTMLEditProbe new];
    [doc.clip addSubview:probe];
    [window makeFirstResponder:probe];
    BOOL dispatch = YES;
    NSArray *keys = @[@"a", @"c", @"v", @"x", @"z", @"z"];
    NSArray *actions = @[@"selectAll:", @"copy:", @"paste:", @"cut:", @"undo:", @"redo:"];
    for (NSUInteger i = 0; i < keys.count; i++) {
        probe.lastAction = nil;
        NSEventModifierFlags flags = NSEventModifierFlagCommand | (i == 5 ? NSEventModifierFlagShift : 0);
        dispatch &= grove_html_route_edit_event(grove_html_edit_test_event(window, keys[i], flags));
        dispatch &= [probe.lastAction isEqualToString:actions[i]];
    }
    results[@"editing_dispatch"] = @(dispatch);
    BOOL ignored = !grove_html_route_edit_event(grove_html_edit_test_event(window, @"a", 0));
    ignored &= !grove_html_route_edit_event(grove_html_edit_test_event(window, @"a", NSEventModifierFlagCommand | NSEventModifierFlagOption));
    ignored &= !grove_html_route_edit_event(grove_html_edit_test_event(window, @"p", NSEventModifierFlagCommand));
    doc.clip.hidden = YES;
    ignored &= !grove_html_route_edit_event(grove_html_edit_test_event(window, @"a", NSEventModifierFlagCommand));
    doc.clip.hidden = NO;
    [window.contentView addSubview:probe];
    ignored &= !grove_html_route_edit_event(grove_html_edit_test_event(window, @"a", NSEventModifierFlagCommand));
    results[@"editing_host_and_hidden_untouched"] = @(ignored);
    [probe removeFromSuperview];
    [window makeFirstResponder:doc.web];
    evaluate(doc.web, @"document.querySelector('#name').focus();document.querySelector('#name').value='Native selection';document.querySelector('#name').setSelectionRange(2,2)");
    pump(0.1);
    BOOL routed = grove_html_route_edit_event(grove_html_edit_test_event(window, @"a", NSEventModifierFlagCommand));
    pump(0.1);
    id selection = evaluate(doc.web, @"(()=>{let e=document.querySelector('#name');return e.selectionStart===0&&e.selectionEnd===e.value.length})()");
    results[@"editing_webkit_select_all"] = @((BOOL)(routed && [selection isEqual:@YES]));
    // Preserve every available pasteboard representation; do not log clipboard
    // contents. These are real WebKit editing actions, not script replacements.
    NSUndoManager *hostUndo = window.undoManager;
    BOOL hostCouldUndo = hostUndo.canUndo, hostCouldRedo = hostUndo.canRedo;
    NSPasteboard *pasteboard = NSPasteboard.generalPasteboard;
    NSMutableArray<NSPasteboardItem *> *savedItems = [NSMutableArray new];
    for (NSPasteboardItem *item in pasteboard.pasteboardItems) {
        NSPasteboardItem *saved = [NSPasteboardItem new];
        for (NSPasteboardType type in item.types) {
            NSData *data = [item dataForType:type];
            if (data) [saved setData:data forType:type];
        }
        [savedItems addObject:saved];
    }
    @try {
        BOOL copied = grove_html_route_edit_event(grove_html_edit_test_event(window, @"c", NSEventModifierFlagCommand));
        pump(0.1);
        copied &= [[pasteboard stringForType:NSPasteboardTypeString] isEqualToString:@"Native selection"];
        BOOL cut = grove_html_route_edit_event(grove_html_edit_test_event(window, @"x", NSEventModifierFlagCommand));
        pump(0.1);
        cut &= [evaluate(doc.web, @"document.querySelector('#name').value") isEqual:@""];
        BOOL undone = grove_html_route_edit_event(grove_html_edit_test_event(window, @"z", NSEventModifierFlagCommand));
        pump(0.1);
        undone &= [evaluate(doc.web, @"document.querySelector('#name').value") isEqual:@"Native selection"];
        BOOL redone = grove_html_route_edit_event(grove_html_edit_test_event(window, @"z", NSEventModifierFlagCommand | NSEventModifierFlagShift));
        pump(0.1);
        redone &= [evaluate(doc.web, @"document.querySelector('#name').value") isEqual:@""];
        [pasteboard clearContents];
        [pasteboard setString:@"Native pasted text" forType:NSPasteboardTypeString];
        BOOL pasted = grove_html_route_edit_event(grove_html_edit_test_event(window, @"v", NSEventModifierFlagCommand));
        pump(0.1);
        pasted &= [evaluate(doc.web, @"document.querySelector('#name').value") isEqual:@"Native pasted text"];
        results[@"editing_webkit_copy_cut_paste"] = @((BOOL)(copied && cut && pasted));
        results[@"editing_webkit_undo_redo"] = @((BOOL)(undone && redone));
        results[@"editing_undo_stack_owned"] = @((BOOL)(doc.web.undoManager == doc.clip.undoManager && doc.web.undoManager != hostUndo && hostUndo.canUndo == hostCouldUndo && hostUndo.canRedo == hostCouldRedo));
        results[@"editing_webkit_detail"] = @{ @"copied": @(copied), @"cut": @(cut), @"pasted": @(pasted), @"undone": @(undone), @"redone": @(redone) };
    } @finally {
        [pasteboard clearContents];
        if (savedItems.count) [pasteboard writeObjects:savedItems];
    }
}

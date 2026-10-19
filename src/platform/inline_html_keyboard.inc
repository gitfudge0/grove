// Included after grove_html_documents(). Local monitoring runs before GPUI's
// performKeyEquivalent:, which otherwise consumes editing keys while a native
// WebKit descendant is the real first responder.
static id grove_html_edit_monitor;

static SEL grove_html_edit_selector(NSEvent *event) {
    if (event.type != NSEventTypeKeyDown) return NULL;
    NSEventModifierFlags modifiers = event.modifierFlags &
        (NSEventModifierFlagCommand | NSEventModifierFlagShift |
         NSEventModifierFlagControl | NSEventModifierFlagOption);
    if (!(modifiers & NSEventModifierFlagCommand) ||
        (modifiers & (NSEventModifierFlagControl | NSEventModifierFlagOption))) return NULL;
    NSString *key = event.charactersIgnoringModifiers.lowercaseString;
    BOOL shift = (modifiers & NSEventModifierFlagShift) != 0;
    if ([key isEqualToString:@"z"]) return shift ? @selector(redo:) : @selector(undo:);
    if (shift) return NULL;
    if ([key isEqualToString:@"a"]) return @selector(selectAll:);
    if ([key isEqualToString:@"c"]) return @selector(copy:);
    if ([key isEqualToString:@"v"]) return @selector(paste:);
    if ([key isEqualToString:@"x"]) return @selector(cut:);
    return NULL;
}

static BOOL grove_html_route_edit_event(NSEvent *event) {
    SEL action = grove_html_edit_selector(event);
    if (!action || !event.window) return NO;
    NSResponder *responder = event.window.firstResponder;
    if (![responder isKindOfClass:NSView.class]) return NO;
    for (GroveHTMLDocument *doc in grove_html_documents()) {
        if (doc.disposed || doc.clip.hiddenOrHasHiddenAncestor ||
            doc.clip.window != event.window ||
            ![(NSView *)responder isDescendantOf:doc.clip]) continue;
        // Explicit targets prevent falling through the host responder chain.
        // WebKit's content responder implements the native editing actions;
        // leave unhandled keys to ordinary AppKit/GPUI dispatch.
        if ([responder respondsToSelector:action] &&
            [NSApp sendAction:action to:responder from:nil]) return YES;
        if ([doc.web respondsToSelector:action] &&
            [NSApp sendAction:action to:doc.web from:nil]) return YES;
        // WebKit exposes its editing undo stack through NSUndoManager rather
        // than undo:/redo: on some macOS releases. Use the clip’s dedicated native editing stack,
        // never the host application or GPUI responder chain.
        NSUndoManager *undo = doc.clip.undoManager;
        if (action == @selector(undo:) && undo.canUndo) { [undo undo]; return YES; }
        if (action == @selector(redo:) && undo.canRedo) { [undo redo]; return YES; }
        return NO;
    }
    return NO;
}

static void grove_html_install_edit_monitor(void) {
    if (grove_html_edit_monitor) return;
    grove_html_edit_monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown
        handler:^NSEvent *(NSEvent *event) {
            return grove_html_route_edit_event(event) ? nil : event;
        }];
}

static void grove_html_remove_edit_monitor_if_unused(void) {
    if (grove_html_documents().count != 0 || !grove_html_edit_monitor) return;
    [NSEvent removeMonitor:grove_html_edit_monitor];
    grove_html_edit_monitor = nil;
}

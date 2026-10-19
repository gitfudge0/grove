// Included after GroveHTMLDocument and grove_html_documents are declared.
// AccessKit owns the content view's synthetic AX tree. Extend only the exact
// content-view instance receiving our native children, preserving its existing
// implementations for everything outside visible HTML surfaces.
#import <objc/runtime.h>

static char grove_html_ax_original_class_key;

static Class grove_html_ax_original_class(NSView *view) {
    NSValue *value = objc_getAssociatedObject(view, &grove_html_ax_original_class_key);
    return value ? (__bridge Class)value.pointerValue : Nil;
}

static BOOL grove_html_ax_owns(GroveHTMLDocument *document, NSView *parent) {
    return !document.disposed && !document.clip.hidden &&
        parent.window && document.clip.window == parent.window &&
        [document.clip isDescendantOf:parent] &&
        !document.clip.hiddenOrHasHiddenAncestor && !NSIsEmptyRect(document.clip.visibleRect);
}

static NSArray *grove_html_ax_children(NSView *view, SEL selector) {
    Class original = grove_html_ax_original_class(view);
    Method method = class_getInstanceMethod(original, selector);
    NSArray *existing = method ? ((id (*)(id, SEL))method_getImplementation(method))(view, selector) : nil;
    NSMutableArray *children = existing ? [existing mutableCopy] : [NSMutableArray new];
    // AppKit flattens ignored NSView containers, retaining WebKit's own native
    // accessibility hierarchy rather than manufacturing a parallel DOM tree.
    for (GroveHTMLDocument *document in grove_html_documents()) {
        if (!grove_html_ax_owns(document, view)) continue;
        for (id child in NSAccessibilityUnignoredChildren(@[document.web])) {
            if (![children containsObject:child]) [children addObject:child];
        }
    }
    return children;
}

static id grove_html_ax_focus(NSView *view, SEL selector) {
    for (GroveHTMLDocument *document in grove_html_documents()) {
        if (!grove_html_ax_owns(document, view)) continue;
        NSResponder *responder = view.window.firstResponder;
        if ([responder isKindOfClass:NSView.class] &&
            [(NSView *)responder isDescendantOf:document.clip]) {
            id focused = [document.web accessibilityFocusedUIElement];
            if (focused) return focused;
        }
    }
    Method method = class_getInstanceMethod(grove_html_ax_original_class(view), selector);
    return method ? ((id (*)(id, SEL))method_getImplementation(method))(view, selector) : nil;
}

static id grove_html_ax_native_hit(NSView *node, NSView *root, NSPoint screenPoint) {
    // Visit the actual native hierarchy front-to-back, including intermediate
    // GPUI render views below the AccessKit-subclassed window content view.
    for (NSView *child in [node.subviews reverseObjectEnumerator]) {
        if (child.hiddenOrHasHiddenAncestor) continue;
        for (GroveHTMLDocument *document in grove_html_documents()) {
            if (document.clip != child || !grove_html_ax_owns(document, root)) continue;
            NSPoint windowPoint = [root.window convertPointFromScreen:screenPoint];
            NSPoint localPoint = [document.clip convertPoint:windowPoint fromView:nil];
            if (!NSPointInRect(localPoint, document.clip.visibleRect)) continue;
            id hit = [document.web accessibilityHitTest:screenPoint];
            if (hit) return hit;
        }
        id hit = grove_html_ax_native_hit(child, root, screenPoint);
        if (hit) return hit;
    }
    return nil;
}

static id grove_html_ax_hit_test(NSView *view, SEL selector, NSPoint screenPoint) {
    // Accessibility points are global bottom-left screen coordinates. Only
    // return WebKit nodes inside their visible native clipping intersection.
    id hit = grove_html_ax_native_hit(view, view, screenPoint);
    if (hit) return hit;
    Method method = class_getInstanceMethod(grove_html_ax_original_class(view), selector);
    return method ? ((id (*)(id, SEL, NSPoint))method_getImplementation(method))(view, selector, screenPoint) : nil;
}

static void grove_html_ax_install(NSView *parent) {
    if (grove_html_ax_original_class(parent)) return;
    Class original = object_getClass(parent);
    NSString *name = [NSString stringWithFormat:@"GroveHTMLAccessibility_%s", class_getName(original)];
    Class subclass = NSClassFromString(name);
    if (!subclass) {
        subclass = objc_allocateClassPair(original, name.UTF8String, 0);
        SEL children = @selector(accessibilityChildren);
        SEL focus = @selector(accessibilityFocusedUIElement);
        SEL hitTest = @selector(accessibilityHitTest:);
        class_addMethod(subclass, children, (IMP)grove_html_ax_children,
                        method_getTypeEncoding(class_getInstanceMethod(original, children)));
        class_addMethod(subclass, focus, (IMP)grove_html_ax_focus,
                        class_getInstanceMethod(original, focus) ?
                            method_getTypeEncoding(class_getInstanceMethod(original, focus)) : "@@:");
        class_addMethod(subclass, hitTest, (IMP)grove_html_ax_hit_test,
                        method_getTypeEncoding(class_getInstanceMethod(original, hitTest)));
        objc_registerClassPair(subclass);
    }
    objc_setAssociatedObject(parent, &grove_html_ax_original_class_key,
        [NSValue valueWithPointer:(__bridge const void *)original], OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    object_setClass(parent, subclass);
    // The subclass stores no document or view references. When all surfaces
    // detach, every method delegates to the original AccessKit implementation;
    // the host can destroy the view normally without a teardown retain cycle.
}

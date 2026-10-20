/* Standalone integration probe: includes the exact production backend. */
#include "../src/platform/inline_html_linux.c"
#include <assert.h>
#include <dlfcn.h>
#include <X11/keysym.h>
static void *xtest;
static int (*fake_motion)(Display *, int, int, int, unsigned long);
static int (*fake_button)(Display *, unsigned int, int, unsigned long);
static int (*fake_key)(Display *, unsigned int, int, unsigned long);
static void click_at(Display *display, int x, int y) {
    fake_motion(display, -1, x, y, 0);
    fake_button(display, 1, True, 0); fake_button(display, 1, False, 0); XSync(display, False);
}
static gchar *evaluation;
static gboolean evaluation_done;
static void evaluated(GObject *web, GAsyncResult *result, gpointer unused) {
    (void)unused;
    GError *error = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(web), result, &error);
    if (error) { fprintf(stderr, "evaluation failed: %s\n", error->message); g_error_free(error); }
    if (value) { evaluation = jsc_value_to_string(value); g_object_unref(value); }
    evaluation_done = TRUE;
}
static void wait_ms(int duration) {
    gint64 deadline = g_get_monotonic_time() + duration * 1000;
    while (g_get_monotonic_time() < deadline) { grove_html_linux_pump(); g_usleep(1000); }
}
static gchar *evaluate(GroveLinuxHtml *doc, const gchar *script) {
    evaluation_done = FALSE; evaluation = NULL;
    webkit_web_view_evaluate_javascript(doc->web, script, -1, NULL, NULL, NULL, evaluated, NULL);
    for (int i = 0; i < 10000 && !evaluation_done; i++) wait_ms(1);
    assert(evaluation_done && evaluation); return evaluation;
}
static void wait_painted(GroveLinuxHtml *doc) {
    gchar *value = evaluate(doc, "window.probePainted=false;document.fonts.ready.then(()=>requestAnimationFrame(()=>requestAnimationFrame(()=>window.probePainted=true)));'scheduled'");
    g_free(value);
    for (int frame = 0; frame < 250; frame++) {
        wait_ms(20);
        value = evaluate(doc, "String(window.probePainted)");
        gboolean painted = g_strcmp0(value, "true") == 0; g_free(value);
        if (painted) return;
    }
    assert(!"WebKit did not present its document frames");
}
static void geometry(GroveLinuxHtml *doc, int x, int y, int width, int height) {
    XWindowAttributes attrs;
    assert(XGetWindowAttributes(doc->display, doc->clip, &attrs));
    assert(attrs.x == x && attrs.y == y && attrs.width == width && attrs.height == height);
}
int main(int argc, char **argv) {
    setbuf(stdout, NULL);
    assert(argc == 2);
    gchar *html = NULL; assert(g_file_get_contents(argv[1], &html, NULL, NULL));
    gdk_set_allowed_backends("x11"); assert(gtk_init_check(NULL, NULL));
    Display *display = gdk_x11_display_get_xdisplay(gdk_display_get_default());
    Window parent = XCreateSimpleWindow(display, DefaultRootWindow(display), 0, 0, 1000, 800, 0, 0, 0);
    XSelectInput(display, parent, ButtonPressMask | KeyPressMask);
    XMapWindow(display, parent); XSync(display, False);
    xtest = dlopen("libXtst.so.6", RTLD_NOW); assert(xtest);
    fake_motion = dlsym(xtest, "XTestFakeMotionEvent");
    fake_button = dlsym(xtest, "XTestFakeButtonEvent");
    fake_key = dlsym(xtest, "XTestFakeKeyEvent");
    assert(fake_motion && fake_button && fake_key);
    GroveLinuxHtml *doc = grove_html_linux_create((unsigned char *)html, strlen(html), parent);
    assert(doc);
    for (int i = 0; i < 15000 && doc->status == 0; i++) wait_ms(1);
    if (doc->status != 1) fprintf(stderr, "unexpected load status %d URI %s\n", doc->status, webkit_web_view_get_uri(doc->web));
    assert(doc->status == 1);
    assert(webkit_web_view_is_ephemeral(doc->web));
    assert(!webkit_settings_get_enable_webrtc(webkit_web_view_get_settings(doc->web)));
    grove_html_linux_show(doc, parent, 10, 20, 600, 400, 40, 80, 300, 160, 1);
    wait_painted(doc);
    geometry(doc, 40, 80, 300, 160);
    Window tree_root, tree_parent, *tree_children = NULL; unsigned tree_count;
    assert(XQueryTree(display, doc->inner, &tree_root, &tree_parent, &tree_children, &tree_count));
    if (tree_children) XFree(tree_children);
    assert(tree_parent == doc->clip);
    assert(XQueryTree(display, doc->clip, &tree_root, &tree_parent, &tree_children, &tree_count));
    if (tree_children) XFree(tree_children);
    assert(tree_parent == parent);
    XWindowAttributes inner; assert(XGetWindowAttributes(display, doc->inner, &inner));
    assert(inner.x == -30 && inner.y == -60 && inner.width == 600 && inner.height == 400);
    gchar *result = evaluate(doc, "JSON.stringify({width:innerWidth,height:innerHeight,counter:window.counter,rtc:typeof RTCPeerConnection,worker:typeof Worker,immutable:!Object.getOwnPropertyDescriptor(globalThis,'RTCPeerConnection').configurable,guard:window.guardOK,blocked:window.blocked})");
    printf("document-probe=%s\n", result); g_free(result);
    click_at(display, 100, 120); wait_ms(150);
    result = evaluate(doc, "String(window.counter)");
    assert(g_strcmp0(result, "1") == 0); g_free(result);
    click_at(display, 100, 155); wait_ms(100);
    unsigned int key = XKeysymToKeycode(display, XK_a);
    fake_key(display, key, True, 0); fake_key(display, key, False, 0); XFlush(display); wait_ms(100);
    result = evaluate(doc, "document.getElementById('entry').value");
    assert(g_strcmp0(result, "a") == 0); g_free(result);
    /* The underlying host receives clicks outside the clipping rectangle. */
    click_at(display, 20, 30);
    XEvent event; assert(XCheckWindowEvent(display, parent, ButtonPressMask, &event));
    wait_ms(50);
    result = evaluate(doc, "String(window.counter)"); assert(g_strcmp0(result, "1") == 0); g_free(result);
    result = evaluate(doc, "const frame=document.createElement('iframe');document.body.append(frame);String(typeof frame.contentWindow.RTCPeerConnection)+'/'+typeof frame.contentWindow.Worker");
    assert(g_strcmp0(result, "undefined/undefined") == 0); g_free(result);
    XSetInputFocus(display, doc->inner, RevertToParent, CurrentTime);
    grove_html_linux_focus_host(parent); XSync(display, False);
    Window focus; int revert; XGetInputFocus(display, &focus, &revert); assert(focus == parent);
    key = XKeysymToKeycode(display, XK_space);
    fake_key(display, key, True, 0); fake_key(display, key, False, 0); XSync(display, False);
    assert(XCheckWindowEvent(display, parent, KeyPressMask, &event));
    grove_html_linux_retire(doc);
    grove_html_linux_show(doc, parent, 10, 20, 600, 400, 40, 80, 300, 160, 1);
    wait_ms(50); assert(doc->visible);
    grove_html_linux_retire(doc); wait_ms(50); assert(!doc->visible);
    grove_html_linux_show(doc, parent, 10, 20, 600, 400, 40, 80, 300, 160, 1);
    grove_html_linux_hide_all(); assert(!doc->visible);
    /* Scaling changes CSS size only through the intended ratio, not clipping. */
    grove_html_linux_show(doc, parent, 20, 40, 1200, 800, 80, 160, 600, 320, 2);
    wait_painted(doc);
    result = evaluate(doc, "String(innerWidth)");
    printf("scaled-css-width=%s\n", result); assert(atoi(result) == 600); g_free(result);
    /* Navigation/popup attempts must leave the retained document unchanged. */
    result = evaluate(doc, "document.getElementById('upload').click();document.getElementById('form').requestSubmit();window.open('https://example.invalid');location.href='https://example.invalid';'requested'");
    g_free(result); wait_ms(250);
    assert(g_strcmp0(webkit_web_view_get_uri(doc->web), document_uri) == 0);
    assert(doc->status == 1);
    grove_html_linux_destroy(doc);
    /* Dispose while asynchronous filter compilation is still in flight. */
    doc = grove_html_linux_create((unsigned char *)html, strlen(html), parent);
    assert(doc); grove_html_linux_retire(doc); grove_html_linux_destroy(doc); wait_ms(250);
    /* The retained session may outlive its native GPUI window. */
    doc = grove_html_linux_create((unsigned char *)html, strlen(html), parent);
    assert(doc);
    XDestroyWindow(display, parent); XSync(display, False);
    grove_html_linux_hide(doc); grove_html_linux_destroy(doc); wait_ms(250);
    g_free(html); dlclose(xtest);
    puts("PASS inline HTML Linux: isolation, clipped layout, interaction, scale, focus, lifetime");
    return 0;
}

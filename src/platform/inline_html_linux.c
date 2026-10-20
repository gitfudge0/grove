/* Public GTK3/WebKitGTK/X11 APIs only. Documents have no host IPC bridge. */
#include <gtk/gtk.h>
#include <gdk/gdkx.h>
#include <webkit2/webkit2.h>
#include <X11/Xlib.h>
#include <math.h>
#include <stdint.h>
#include <glib/gstdio.h>

typedef struct {
    unsigned refs;
    gboolean disposed, initial, visible;
    int status;
    uint64_t generation;
    Display *display;
    Window parent, clip, inner;
    GtkWidget *host;
    WebKitWebView *web;
    WebKitUserContentFilterStore *store;
    GCancellable *cancel;
    gchar *source, *filter_dir;
} GroveLinuxHtml;
static GList *documents;
static gboolean initialized;
static const char *document_uri = "grove-html://document/";
static void release(GroveLinuxHtml *doc) {
    if (--doc->refs) return;
    if (doc->filter_dir) {
        GDir *dir = g_dir_open(doc->filter_dir, 0, NULL);
        if (dir) { const gchar *name; while ((name = g_dir_read_name(dir))) {
            gchar *path = g_build_filename(doc->filter_dir, name, NULL);
            g_unlink(path); g_free(path);
        } g_dir_close(dir); }
        g_rmdir(doc->filter_dir);
    }
    g_free(doc->source); g_free(doc->filter_dir); g_free(doc);
}
static gboolean policy(WebKitWebView *web, WebKitPolicyDecision *decision,
                       WebKitPolicyDecisionType type, GroveLinuxHtml *doc) {
    if (type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(
            WEBKIT_NAVIGATION_POLICY_DECISION(decision));
        const gchar *uri = webkit_uri_request_get_uri(webkit_navigation_action_get_request(action));
        if (doc->initial && g_strcmp0(uri, document_uri) == 0 &&
            webkit_navigation_action_get_navigation_type(action) == WEBKIT_NAVIGATION_TYPE_OTHER) {
            doc->initial = FALSE; webkit_policy_decision_use(decision); return TRUE;
        }
        /* Local fragment navigation is safe; every changed document is denied. */
        const gchar *current = webkit_web_view_get_uri(web);
        gchar *requested_base = g_strdup(uri), *current_base = g_strdup(current);
        gchar *fragment = requested_base ? strchr(requested_base, '#') : NULL;
        gchar *old_fragment = current_base ? strchr(current_base, '#') : NULL;
        gboolean changes_fragment = (fragment || old_fragment) && g_strcmp0(fragment, old_fragment);
        if (fragment) *fragment = '\0';
        if (old_fragment) *old_fragment = '\0';
        gboolean same = changes_fragment && g_strcmp0(requested_base, document_uri) == 0 &&
                        g_strcmp0(requested_base, current_base) == 0;
        g_free(requested_base); g_free(current_base);
        if (same) { webkit_policy_decision_use(decision); return TRUE; }
    }
    /* load_html's synthetic response is the only admissible resource response. */
    if (type == WEBKIT_POLICY_DECISION_TYPE_RESPONSE) {
        WebKitURIRequest *request = webkit_response_policy_decision_get_request(WEBKIT_RESPONSE_POLICY_DECISION(decision));
        if (g_strcmp0(webkit_uri_request_get_uri(request), document_uri) == 0) {
            webkit_policy_decision_use(decision); return TRUE;
        }
    }
    webkit_policy_decision_ignore(decision); return TRUE;
}
static gboolean permission(WebKitWebView *web, WebKitPermissionRequest *request, gpointer unused) {
    (void)web; (void)unused; webkit_permission_request_deny(request); return TRUE;
}
static void download(WebKitWebContext *context, WebKitDownload *download, gpointer unused) {
    (void)context; (void)unused; webkit_download_cancel(download);
}
static WebKitWebView *popup(WebKitWebView *web, WebKitNavigationAction *action, gpointer unused) {
    (void)web; (void)action; (void)unused; return NULL;
}
static gboolean chooser(WebKitWebView *web, WebKitFileChooserRequest *request, gpointer unused) {
    (void)web; (void)unused; webkit_file_chooser_request_cancel(request); return TRUE;
}
static gboolean dialog(WebKitWebView *web, WebKitScriptDialog *dialog, gpointer unused) {
    (void)web; (void)unused; (void)dialog;
    /* Synchronous handled return closes dialogs without exposing application UI. */
    return TRUE;
}
static gboolean context_menu(WebKitWebView *web, WebKitContextMenu *menu,
                              GdkEvent *event, WebKitHitTestResult *hit, gpointer unused) {
    (void)web; (void)menu; (void)event; (void)hit; (void)unused; return TRUE;
}
static gboolean fullscreen(WebKitWebView *web, gpointer unused) { (void)web; (void)unused; return TRUE; }
static gboolean printing(WebKitWebView *web, WebKitPrintOperation *operation, gpointer unused) {
    (void)web; (void)operation; (void)unused; return TRUE;
}
static void loaded(WebKitWebView *web, WebKitLoadEvent event, GroveLinuxHtml *doc) {
    (void)web; if (!doc->disposed && event == WEBKIT_LOAD_FINISHED && doc->status != 2) doc->status = 1;
}
static gboolean failed(WebKitWebView *web, WebKitLoadEvent event, gchar *uri, GError *error, GroveLinuxHtml *doc) {
    (void)web; (void)event; (void)uri;
    if (!doc->disposed && !g_error_matches(error, WEBKIT_POLICY_ERROR, WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE)) doc->status = 2;
    return TRUE;
}
static void terminated(WebKitWebView *web, WebKitWebProcessTerminationReason reason, GroveLinuxHtml *doc) {
    (void)web; (void)reason; if (!doc->disposed) doc->status = 2;
}
static void filter_ready(GObject *store, GAsyncResult *result, gpointer data) {
    GroveLinuxHtml *doc = data;
    GError *error = NULL;
    WebKitUserContentFilter *filter = webkit_user_content_filter_store_save_finish(
        WEBKIT_USER_CONTENT_FILTER_STORE(store), result, &error);
    if (!doc->disposed) {
        if (filter && !error) {
            webkit_user_content_manager_add_filter(webkit_web_view_get_user_content_manager(doc->web), filter);
            doc->initial = TRUE;
            webkit_web_view_load_html(doc->web, doc->source, document_uri);
        } else doc->status = 2;
    }
    if (filter) webkit_user_content_filter_unref(filter);
    g_clear_error(&error); release(doc);
}
void *grove_html_linux_create(const unsigned char *bytes, size_t length, unsigned long parent) {
    if (!initialized) {
        gdk_set_allowed_backends("x11");
        if (!gtk_init_check(NULL, NULL)) return NULL;
        initialized = TRUE;
    }
    GdkDisplay *gdk = gdk_display_get_default();
    if (!GDK_IS_X11_DISPLAY(gdk)) return NULL;
    GroveLinuxHtml *doc = g_new0(GroveLinuxHtml, 1);
    doc->refs = 1; doc->display = gdk_x11_display_get_xdisplay(gdk); doc->parent = parent;
    doc->source = g_strndup((const gchar *)bytes, length);
    doc->clip = XCreateSimpleWindow(doc->display, parent, 0, 0, 1, 1, 0, 0, 0);
    /* GTK owns its native window and event registration. Reparent only its XID;
       the outer X11 child performs clipping without altering the HTML viewport. */
    doc->host = gtk_window_new(GTK_WINDOW_POPUP);
    gtk_window_set_accept_focus(GTK_WINDOW(doc->host), TRUE);
    gtk_widget_realize(doc->host);
    doc->inner = gdk_x11_window_get_xid(gtk_widget_get_window(doc->host));
    XReparentWindow(doc->display, doc->inner, doc->clip, 0, 0);
    WebKitWebContext *context = webkit_web_context_new_ephemeral();
    g_signal_connect(context, "download-started", G_CALLBACK(download), NULL);
    doc->web = WEBKIT_WEB_VIEW(webkit_web_view_new_with_context(context));
    g_object_unref(context);
    WebKitSettings *settings = webkit_web_view_get_settings(doc->web);
    g_object_set(settings, "enable-webrtc", FALSE, "enable-developer-extras", FALSE,
        "javascript-can-open-windows-automatically", FALSE,
        "javascript-can-access-clipboard", FALSE, "enable-html5-database", FALSE,
        "enable-html5-local-storage", FALSE, "allow-file-access-from-file-urls", FALSE,
        "allow-universal-access-from-file-urls", FALSE, "enable-fullscreen", FALSE,
        "media-playback-requires-user-gesture", TRUE, NULL);
    const gchar *guard = "for(const name of ['RTCPeerConnection','webkitRTCPeerConnection','RTCIceGatherer','WebTransport','Worker','SharedWorker','ServiceWorker','Notification']){try{Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});}catch{}}";
    WebKitUserScript *script = webkit_user_script_new(guard, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES,
        WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, NULL, NULL);
    webkit_user_content_manager_add_script(webkit_web_view_get_user_content_manager(doc->web), script);
    webkit_user_script_unref(script);
    g_signal_connect(doc->web, "decide-policy", G_CALLBACK(policy), doc);
    g_signal_connect(doc->web, "permission-request", G_CALLBACK(permission), NULL);
    g_signal_connect(doc->web, "create", G_CALLBACK(popup), NULL);
    g_signal_connect(doc->web, "run-file-chooser", G_CALLBACK(chooser), NULL);
    g_signal_connect(doc->web, "script-dialog", G_CALLBACK(dialog), NULL);
    g_signal_connect(doc->web, "context-menu", G_CALLBACK(context_menu), NULL);
    g_signal_connect(doc->web, "enter-fullscreen", G_CALLBACK(fullscreen), NULL);
    g_signal_connect(doc->web, "print", G_CALLBACK(printing), NULL);
    g_signal_connect(doc->web, "load-changed", G_CALLBACK(loaded), doc);
    g_signal_connect(doc->web, "load-failed", G_CALLBACK(failed), doc);
    g_signal_connect(doc->web, "web-process-terminated", G_CALLBACK(terminated), doc);
    gtk_container_add(GTK_CONTAINER(doc->host), GTK_WIDGET(doc->web));
    gtk_widget_show_all(doc->host);
    XUnmapWindow(doc->display, doc->clip);
    documents = g_list_prepend(documents, doc);
    doc->filter_dir = g_dir_make_tmp("grove-html-filter-XXXXXX", NULL);
    if (!doc->filter_dir) { doc->status = 2; return doc; }
    doc->store = webkit_user_content_filter_store_new(doc->filter_dir);
    doc->cancel = g_cancellable_new();
    const char *rules = "[{\"trigger\":{\"url-filter\":\".*\"},\"action\":{\"type\":\"block\"}},{\"trigger\":{\"url-filter\":\"^grove-html://document/\"},\"action\":{\"type\":\"ignore-previous-rules\"}},{\"trigger\":{\"url-filter\":\"^about:blank\"},\"action\":{\"type\":\"ignore-previous-rules\"}},{\"trigger\":{\"url-filter\":\"^data:\"},\"action\":{\"type\":\"ignore-previous-rules\"}}]";
    GBytes *source = g_bytes_new_static(rules, strlen(rules));
    doc->refs++;
    webkit_user_content_filter_store_save(doc->store, "grove-self-contained-html-v1", source,
        doc->cancel, filter_ready, doc);
    g_bytes_unref(source);
    return doc;
}
int grove_html_linux_status(void *handle) { return ((GroveLinuxHtml *)handle)->status; }
static void focus_document(GroveLinuxHtml *doc) {
        if (!doc->visible) return;
        unsigned long parent = doc->parent;
        Window focus; int revert;
        XGetInputFocus(doc->display, &focus, &revert);
        if (focus == None || focus == PointerRoot) return;
        Window current = focus;
        for (int depth = 0; current && depth < 32; depth++) {
            if (current == doc->clip || current == doc->inner) {
                XSetInputFocus(doc->display, parent, RevertToParent, CurrentTime); XFlush(doc->display); break;
            }
            Window root, ancestor, *children = NULL; unsigned count;
            if (!XQueryTree(doc->display, current, &root, &ancestor, &children, &count)) break;
            if (children) XFree(children);
            if (ancestor == current || current == root) break;
            current = ancestor;
        }
}
void grove_html_linux_focus_host(unsigned long parent) {
    for (GList *item = documents; item; item = item->next) {
        GroveLinuxHtml *doc = item->data;
        if (doc->parent == parent) {
            gdk_x11_display_error_trap_push(gdk_display_get_default());
            focus_document(doc);
            gdk_x11_display_error_trap_pop_ignored(gdk_display_get_default());
        }
    }
}
void grove_html_linux_hide(void *handle) {
    GroveLinuxHtml *doc = handle;
    if (doc->disposed) return;
    gdk_x11_display_error_trap_push(gdk_display_get_default());
    focus_document(doc);
    XUnmapWindow(doc->display, doc->clip); XFlush(doc->display); doc->visible = FALSE;
    gdk_x11_display_error_trap_pop_ignored(gdk_display_get_default());
}
typedef struct { GroveLinuxHtml *doc; uint64_t generation; } Retirement;
static gboolean retire(gpointer data) {
    Retirement *retirement = data;
    if (!retirement->doc->disposed && retirement->generation == retirement->doc->generation)
        grove_html_linux_hide(retirement->doc);
    return G_SOURCE_REMOVE;
}
static void retired(gpointer data) { Retirement *r = data; release(r->doc); g_free(r); }
void grove_html_linux_retire(void *handle) {
    GroveLinuxHtml *doc = handle;
    Retirement *retirement = g_new(Retirement, 1);
    retirement->doc = doc; retirement->generation = doc->generation; doc->refs++;
    g_idle_add_full(G_PRIORITY_DEFAULT_IDLE, retire, retirement, retired);
}
void grove_html_linux_show(void *handle, unsigned long parent, double x, double y,
    double width, double height, double clip_x, double clip_y, double clip_width, double clip_height, double scale) {
    GroveLinuxHtml *doc = handle;
    if (doc->disposed || doc->status != 1 || width <= 0 || height <= 0 || clip_width <= 0 || clip_height <= 0) {
        grove_html_linux_hide(doc); return;
    }
    doc->generation++;
    if (doc->parent != parent) {
        grove_html_linux_hide(doc); XReparentWindow(doc->display, doc->clip, parent, 0, 0); doc->parent = parent;
    }
    int left = (int)ceil(clip_x), top = (int)ceil(clip_y);
    int right = (int)floor(clip_x + clip_width), bottom = (int)floor(clip_y + clip_height);
    if (right <= left || bottom <= top) { grove_html_linux_hide(doc); return; }
    int gtk_scale = gtk_widget_get_scale_factor(doc->host);
    webkit_web_view_set_zoom_level(doc->web, scale / gtk_scale);
    gtk_window_resize(GTK_WINDOW(doc->host), MAX(1, (int)ceil(width / gtk_scale)), MAX(1, (int)ceil(height / gtk_scale)));
    XMoveResizeWindow(doc->display, doc->clip, left, top, right-left, bottom-top);
    XMoveResizeWindow(doc->display, doc->inner, (int)round(x)-left, (int)round(y)-top,
        MAX(1, (int)ceil(width)), MAX(1, (int)ceil(height)));
    XMapWindow(doc->display, doc->inner); XMapRaised(doc->display, doc->clip); XFlush(doc->display);
    doc->visible = TRUE;
}
void grove_html_linux_hide_all(void) {
    for (GList *item = documents; item; item = item->next) grove_html_linux_hide(item->data);
}
void grove_html_linux_pump(void) {
    if (!initialized) return;
    /* Bound each turn; a busy document must not starve GPUI's input/render loop. */
    for (int budget = 0; budget < 32 && g_main_context_pending(NULL); budget++)
        g_main_context_iteration(NULL, FALSE);
}
void grove_html_linux_destroy(void *handle) {
    GroveLinuxHtml *doc = handle;
    grove_html_linux_hide(doc); doc->disposed = TRUE;
    documents = g_list_remove(documents, doc);
    if (doc->cancel) g_cancellable_cancel(doc->cancel);
    webkit_web_view_stop_loading(doc->web);
    g_signal_handlers_disconnect_by_data(doc->web, doc);
    gdk_x11_display_error_trap_push(gdk_display_get_default());
    gtk_widget_destroy(doc->host);
    XDestroyWindow(doc->display, doc->clip); XFlush(doc->display);
    gdk_x11_display_error_trap_pop_ignored(gdk_display_get_default());
    g_clear_object(&doc->cancel); g_clear_object(&doc->store);
    release(doc);
}

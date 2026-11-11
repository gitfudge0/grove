/* Main-thread-only desktop text preference bridge. This intentionally does not
 * inspect monitor DPI or GDK_SCALE: those belong to display scaling. GSettings
 * subscriptions work before GTK/WebKit has ever been initialized. */
#include <gio/gio.h>

static GSettings *text_settings;
static gboolean initialized;
static double text_scale = 1.0;

static void text_scale_changed(GSettings *settings, const gchar *key, gpointer unused) {
    (void)unused;
    text_scale = g_settings_get_double(settings, key);
}

double grove_text_scale_init(void) {
    if (initialized) return text_scale;
    initialized = TRUE;
    GSettingsSchemaSource *source = g_settings_schema_source_get_default();
    if (!source) return text_scale;
    GSettingsSchema *schema = g_settings_schema_source_lookup(
        source, "org.gnome.desktop.interface", TRUE);
    if (!schema) return text_scale;
    if (g_settings_schema_has_key(schema, "text-scaling-factor")) {
        /* Retain the settings and subscription for the application lifetime. */
        text_settings = g_settings_new_full(schema, NULL, NULL);
        g_signal_connect(text_settings, "changed::text-scaling-factor",
                         G_CALLBACK(text_scale_changed), NULL);
        /* Reading after connecting also activates the backend subscription. */
        text_scale_changed(text_settings, "text-scaling-factor", NULL);
    }
    g_settings_schema_unref(schema);
    return text_scale;
}

double grove_text_scale_poll(void) {
    grove_text_scale_init();
    /* Bound foreground work even if GTK/WebKit continually enqueues events. */
    for (unsigned i = 0; i < 64 && g_main_context_pending(NULL); ++i)
        g_main_context_iteration(NULL, FALSE);
    return text_scale;
}

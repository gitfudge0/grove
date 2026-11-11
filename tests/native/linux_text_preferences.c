/* Exercises the production GSettings subscription across processes, without GTK. */
#include <gio/gio.h>
#include <math.h>

double grove_text_scale_init(void);
double grove_text_scale_poll(void);

static void set_scale(const char *command) {
    gint status = 0;
    GError *error = NULL;
    g_assert_true(g_spawn_command_line_sync(command, NULL, NULL, &status, &error));
    g_assert_no_error(error);
    g_assert_true(g_spawn_check_wait_status(status, &error));
    g_assert_no_error(error);
}
static void await_scale(double expected) {
    gint64 deadline = g_get_monotonic_time() + 5 * G_TIME_SPAN_SECOND;
    while (fabs(grove_text_scale_poll() - expected) > 0.00001 &&
           g_get_monotonic_time() < deadline)
        g_usleep(10000);
    g_assert_cmpfloat_with_epsilon(grove_text_scale_poll(), expected, 0.00001);
}
int main(int argc, char **argv) {
    (void)argv;
    if (argc > 1) {
        g_assert_cmpfloat(grove_text_scale_init(), ==, 1.0);
        g_assert_cmpfloat(grove_text_scale_poll(), ==, 1.0);
        g_print("missing schema fallback: PASS\n");
        return 0;
    }
    set_scale("gsettings set org.gnome.desktop.interface text-scaling-factor 1.25");
    g_assert_cmpfloat(grove_text_scale_init(), ==, 1.25);
    g_assert_cmpfloat(grove_text_scale_init(), ==, 1.25);
    set_scale("gsettings set org.gnome.desktop.interface text-scaling-factor 1.5");
    await_scale(1.5);
    set_scale("gsettings set org.gnome.desktop.interface text-scaling-factor 0.8182");
    await_scale(0.8182);
    set_scale("gsettings reset org.gnome.desktop.interface text-scaling-factor");
    await_scale(1.0);
    g_print("startup, live increase/decrease, reset before GTK: PASS\n");
    return 0;
}

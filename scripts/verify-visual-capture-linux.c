/* Production WebKitGTK screenshot capture, dimensions, policy and focus probe.
   cc scripts/verify-visual-capture-linux.c -o /tmp/grove-capture-check \
      $(pkg-config --cflags --libs webkit2gtk-4.1 gtk+-3.0 x11 xext) -lm
   /tmp/grove-capture-check /absolute/task-owned-output-directory */
#include "../src/platform/inline_html_linux.c"
#include <stdio.h>
static void pump_ms(int ms) {
    gint64 end = g_get_monotonic_time() + (gint64)ms * 1000;
    while (g_get_monotonic_time() < end) { grove_html_linux_pump(); g_usleep(1000); }
}
int main(int argc, char **argv) {
    if (argc != 2 || !g_path_is_absolute(argv[1])) return 2;
    g_mkdir_with_parents(argv[1], 0700);
    for (int pass = 0; pass < 2; pass++) {
        unsigned width = pass ? 360 : 728;
        const char *source = "<!doctype html><html><head><meta http-equiv='Content-Security-Policy' content=\"default-src 'none';style-src 'unsafe-inline';script-src 'unsafe-inline';connect-src 'none'\"><style>body{margin:0;display:flow-root;background:#123456;color:white;font:16px sans-serif}input{font:inherit}.bar{height:24px;background:#4bb;margin:16px 0;width:70%}</style></head><body><h1>Native screenshot preview</h1><div class='bar'></div><input value='Retained input'><script>globalThis.__groveCaptureConsole=[];fetch('https://example.com/blocked').catch(()=>__groveCaptureConsole.push({level:'error',text:'Network blocked'}));</script></body></html>";
        GroveLinuxHtml *doc = grove_html_linux_capture_create((const unsigned char *)source, strlen(source), width, 560);
        if (!doc) return 3;
        Window focus_before; int revert; XGetInputFocus(doc->display, &focus_before, &revert);
        gint64 deadline = g_get_monotonic_time() + 15 * G_TIME_SPAN_SECOND;
        int length = 0;
        pump_ms(300);
        while (g_get_monotonic_time() < deadline && !(length = grove_html_linux_capture_poll(doc))) pump_ms(16);
        Window focus_after; XGetInputFocus(doc->display, &focus_after, &revert);
        if (length <= 0 || focus_before != focus_after) { fprintf(stderr, "capture failed length=%d focus_before=%lu focus_after=%lu\n", length, focus_before, focus_after); return 4; }
        unsigned char *data = g_malloc(length); grove_html_linux_capture_copy(doc, data, length);
        if (length < 24 || memcmp(data, "\211PNG\r\n\032\n", 8)) return 5;
        unsigned pixels = ((unsigned)data[16] << 24) | ((unsigned)data[17] << 16) | ((unsigned)data[18] << 8) | data[19];
        if (pixels < width || pixels > width * 3) return 6;
        const char *metadata = grove_html_linux_capture_metadata(doc);
        if (!metadata || !strstr(metadata, "Network blocked")) return 7;
        gchar *name = g_strdup_printf("%s/native-preview-%u.png", argv[1], width);
        gboolean saved = g_file_set_contents(name, (char *)data, length, NULL);
        printf("width=%u pixel_width=%u png_bytes=%d focus_preserved=true metadata=%s\n", width, pixels, length, metadata);
        g_free(name); g_free(data); grove_html_linux_destroy(doc);
        if (!saved) return 8;
    }
    const char *source = "<html><body>Cancelled</body></html>";
    void *cancelled = grove_html_linux_capture_create((const unsigned char *)source, strlen(source), 360, 560);
    if (!cancelled) return 9;
    grove_html_linux_destroy(cancelled); pump_ms(300);
    printf("cancelled_load_teardown=passed\n");
    return 0;
}

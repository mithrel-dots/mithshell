#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <wayland-client.h>
#include "virtual-pointer.h"

static struct zwlr_virtual_pointer_manager_v1 *manager;
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    (void)version;
    if (strcmp(interface, "zwlr_virtual_pointer_manager_v1") == 0)
        manager = wl_registry_bind(registry, name,
                                  &zwlr_virtual_pointer_manager_v1_interface, 1);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
int main(int argc, char **argv) {
    assert(argc == 5 || argc == 6);
    struct wl_display *display = wl_display_connect(NULL);
    assert(display);
    struct wl_registry *registry = wl_display_get_registry(display);
    const struct wl_registry_listener listener = {global, removed};
    wl_registry_add_listener(registry, &listener, NULL);
    wl_display_roundtrip(display);
    assert(manager);
    struct zwlr_virtual_pointer_v1 *pointer =
        zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, 1, atoi(argv[1]), atoi(argv[2]),
                                            atoi(argv[3]), atoi(argv[4]));
    zwlr_virtual_pointer_v1_frame(pointer);
    wl_display_roundtrip(display);
    usleep(80000);
    if (argc == 6) {
        zwlr_virtual_pointer_v1_destroy(pointer);
        wl_display_roundtrip(display);
        wl_display_disconnect(display);
        return 0;
    }
    zwlr_virtual_pointer_v1_button(pointer, 2, 272, 1);
    zwlr_virtual_pointer_v1_frame(pointer);
    wl_display_roundtrip(display);
    usleep(80000);
    zwlr_virtual_pointer_v1_button(pointer, 3, 272, 0);
    zwlr_virtual_pointer_v1_frame(pointer);
    wl_display_roundtrip(display);
    usleep(80000);
    zwlr_virtual_pointer_v1_destroy(pointer);
    wl_display_roundtrip(display);
    wl_display_disconnect(display);
}

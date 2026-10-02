/* Optional, isolated-compositor-only workaround for Aquamarine binding fixed
 * protocol versions newer than those advertised by Weston. This changes only
 * the nested parent connection, never Hyprland's input routing or GTK clients.
 * GNU C is required for transparent forwarding of the variadic marshaller. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdarg.h>
#include <stdint.h>
#include <string.h>
#include <wayland-client.h>

struct wl_proxy *wl_proxy_marshal_flags(struct wl_proxy *proxy, uint32_t opcode,
        const struct wl_interface *interface, uint32_t version, uint32_t flags, ...) {
    void *args = __builtin_apply_args();
    void *real = dlsym(RTLD_NEXT, "wl_proxy_marshal_flags");
    uint32_t limit = interface && strcmp(interface->name, "wl_seat") == 0 ? 7 : 5;
    if (strcmp(wl_proxy_get_class(proxy), "wl_registry") == 0 && opcode == 0 && interface &&
        (strcmp(interface->name, "wl_compositor") == 0 || strcmp(interface->name, "wl_seat") == 0 ||
         strcmp(interface->name, "xdg_wm_base") == 0) && version > limit) {
        va_list ap;
        va_start(ap, flags);
        uint32_t name = va_arg(ap, uint32_t);
        const char *name_string = va_arg(ap, const char *);
        (void)va_arg(ap, uint32_t);
        void *id = va_arg(ap, void *);
        va_end(ap);
        typedef struct wl_proxy *(*func)(struct wl_proxy *, uint32_t,
                const struct wl_interface *, uint32_t, uint32_t, ...);
        return ((func)real)(proxy, opcode, interface, limit, flags, name, name_string, limit, id);
    }
    __builtin_return(__builtin_apply(real, args, 256));
}

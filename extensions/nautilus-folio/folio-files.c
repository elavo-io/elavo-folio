/* In-process bridge for Folio's global menu.
 *
 * Nautilus keeps the actions Folio needs on the focused window, not on the
 * session bus. This module owns io.elavo.FolioFiles and runs those actions
 * when the panel asks.
 *
 * GTK 4 has no public call that returns a widget's inserted action group, so
 * view.* and slot.* are read from the NautilusFilesView and NautilusWindowSlot
 * that own them. The field offset is found once, by scanning the object for
 * the GActionGroup that holds a known action, and checked against
 * /proc/self/maps before it is dereferenced. win.* actions live on the
 * application window. toolbar.edit-location is a widget-class action.
 *
 * The module registers no Nautilus type. Nautilus still loads it, then calls
 * g_module_make_resident, so the bus name lives as long as Files does.
 */

#include <dlfcn.h>
#include <gtk/gtk.h>
#include <stdio.h>
#include <string.h>

#define FOLIO_BUS_NAME "io.elavo.FolioFiles"
#define FOLIO_OBJECT_PATH "/io/elavo/FolioFiles"

static const char introspection[] =
    "<node>"
    "  <interface name='io.elavo.FolioFiles'>"
    "    <method name='Activate'>"
    "      <arg type='s' name='action' direction='in'/>"
    "      <arg type='av' name='target' direction='in'/>"
    "      <arg type='b' name='ok' direction='out'/>"
    "    </method>"
    "    <method name='Enabled'>"
    "      <arg type='as' name='actions' direction='in'/>"
    "      <arg type='ab' name='enabled' direction='out'/>"
    "    </method>"
    "  </interface>"
    "</node>";

typedef struct _AdwTabView AdwTabView;
typedef struct _AdwTabPage AdwTabPage;

typedef GType (*AdwGetType)(void);
typedef gboolean (*AdwSelect)(AdwTabView *view);
typedef AdwTabPage *(*AdwSelected)(AdwTabView *view);
typedef int (*AdwPosition)(AdwTabView *view, AdwTabPage *page);
typedef int (*AdwNPages)(AdwTabView *view);

static struct {
    gboolean tried;
    gboolean ok;
    AdwGetType get_type;
    AdwSelect previous;
    AdwSelect next;
    AdwSelected selected;
    AdwPosition position;
    AdwNPages n_pages;
} adw;

static guint name_owner;
static GDBusNodeInfo *info;
static guint registration;

typedef struct {
    unsigned long start;
    unsigned long end;
} MapRange;

static MapRange *maps;
static int n_maps;

static void
load_maps(void)
{
    FILE *file = fopen("/proc/self/maps", "r");
    char line[512];

    g_free(maps);
    maps = NULL;
    n_maps = 0;
    if (!file)
        return;
    while (fgets(line, sizeof line, file)) {
        unsigned long start = 0;
        unsigned long end = 0;
        if (sscanf(line, "%lx-%lx", &start, &end) != 2)
            continue;
        maps = g_renew(MapRange, maps, n_maps + 1);
        maps[n_maps].start = start;
        maps[n_maps].end = end;
        n_maps++;
    }
    fclose(file);
}

static gboolean
readable(const void *pointer, size_t length)
{
    unsigned long start = (unsigned long)pointer;
    unsigned long end = start + length;
    int i;

    if (!pointer || end < start)
        return FALSE;
    if (!maps)
        load_maps();
    for (i = 0; i < n_maps; i++) {
        if (start >= maps[i].start && end <= maps[i].end)
            return TRUE;
    }
    return FALSE;
}

static gboolean
load_adw(void)
{
    if (adw.tried)
        return adw.ok;
    adw.tried = TRUE;
    adw.get_type = dlsym(RTLD_DEFAULT, "adw_tab_view_get_type");
    adw.previous = dlsym(RTLD_DEFAULT, "adw_tab_view_select_previous_page");
    adw.next = dlsym(RTLD_DEFAULT, "adw_tab_view_select_next_page");
    adw.selected = dlsym(RTLD_DEFAULT, "adw_tab_view_get_selected_page");
    adw.position = dlsym(RTLD_DEFAULT, "adw_tab_view_get_page_position");
    adw.n_pages = dlsym(RTLD_DEFAULT, "adw_tab_view_get_n_pages");
    adw.ok = adw.get_type && adw.previous && adw.next && adw.selected && adw.position && adw.n_pages;
    return adw.ok;
}

static GtkWidget *
active_window(void)
{
    GApplication *application = g_application_get_default();
    GtkWindow *window;
    GList *windows;

    if (!GTK_IS_APPLICATION(application))
        return NULL;
    window = gtk_application_get_active_window(GTK_APPLICATION(application));
    if (window)
        return GTK_WIDGET(window);
    windows = gtk_application_get_windows(GTK_APPLICATION(application));
    window = windows ? windows->data : NULL;
    g_list_free(windows);
    return GTK_WIDGET(window);
}

/* The group Nautilus stores on `owner` that contains `probe` (for example
 * "new-folder" on a NautilusFilesView). The offset is remembered per type. */
static GActionGroup *
owned_group(GObject *owner, const char *probe)
{
    static GType cached_type[4];
    static gsize cached_offset[4];
    static int n_cached;
    GType type;
    int slot;
    gsize offset;

    if (!owner)
        return NULL;
    type = G_OBJECT_TYPE(owner);
    for (slot = 0; slot < n_cached; slot++) {
        if (cached_type[slot] == type) {
            GActionGroup *group = *(GActionGroup **)((char *)owner + cached_offset[slot]);
            return G_IS_ACTION_GROUP(group) ? group : NULL;
        }
    }

    load_maps();
    for (offset = sizeof(void *); offset < 4096; offset += sizeof(void *)) {
        GTypeClass *klass;
        void *candidate;

        if (!readable((char *)owner + offset, sizeof(void *)))
            break;
        candidate = *(void **)((char *)owner + offset);
        if (!candidate || ((unsigned long)candidate & (sizeof(void *) - 1)) != 0)
            continue;
        if (!readable(candidate, sizeof(void *)))
            continue;
        klass = *(GTypeClass **)candidate;
        if (!readable(klass, sizeof(GTypeClass)))
            continue;
        /* A pointer field can land in mapped memory that is not an object.
           An unregistered type id is not an action group, and asking before
           the type check keeps that case from walking a bogus class. */
        if (klass->g_type == 0 || g_type_name(klass->g_type) == NULL)
            continue;
        if (!G_IS_ACTION_GROUP(candidate))
            continue;
        if (!g_action_group_has_action(candidate, probe))
            continue;
        if (n_cached < 4) {
            cached_type[n_cached] = type;
            cached_offset[n_cached] = offset;
            n_cached++;
            g_message("folio files: %s action group at offset %zu", g_type_name(type), offset);
        }
        return candidate;
    }
    return NULL;
}

static GtkWidget *
find_typed(GtkWidget *widget, GType type)
{
    GtkWidget *child;
    GtkWidget *focus;

    if (type == G_TYPE_INVALID || !widget)
        return NULL;
    focus = gtk_widget_get_root(widget) ? gtk_root_get_focus(gtk_widget_get_root(widget)) : NULL;
    for (child = focus; child; child = gtk_widget_get_parent(child)) {
        if (g_type_check_instance_is_a((GTypeInstance *)child, type))
            return child;
    }
    if (g_type_check_instance_is_a((GTypeInstance *)widget, type))
        return widget;
    for (child = gtk_widget_get_first_child(widget); child; child = gtk_widget_get_next_sibling(child)) {
        GtkWidget *found = find_typed(child, type);
        if (found)
            return found;
    }
    return NULL;
}

static GtkWidget *
find_class_action(GtkWidget *widget, const char *name)
{
    GtkWidgetClass *klass = GTK_WIDGET_GET_CLASS(widget);
    GtkWidget *child;
    guint index;

    for (index = 0;; index++) {
        GType owner = G_TYPE_INVALID;
        const char *action_name = NULL;
        const GVariantType *parameter = NULL;
        const char *property = NULL;

        if (!gtk_widget_class_query_action(klass, index, &owner, &action_name, &parameter, &property))
            break;
        if (g_strcmp0(action_name, name) == 0)
            return widget;
    }
    for (child = gtk_widget_get_first_child(widget); child; child = gtk_widget_get_next_sibling(child)) {
        GtkWidget *found = find_class_action(child, name);
        if (found)
            return found;
    }
    return NULL;
}

static GActionGroup *
group_for(GtkWidget *window, const char *prefix)
{
    GtkWidget *owner = NULL;
    const char *probe = NULL;

    if (g_str_equal(prefix, "win"))
        return G_ACTION_GROUP(window);
    if (g_str_equal(prefix, "view")) {
        owner = find_typed(window, g_type_from_name("NautilusFilesView"));
        probe = "new-folder";
    } else if (g_str_equal(prefix, "slot")) {
        owner = find_typed(window, g_type_from_name("NautilusWindowSlot"));
        probe = "back";
    }
    return owner ? owned_group(G_OBJECT(owner), probe) : NULL;
}

static gboolean
split_action(const char *detailed, char **prefix, const char **name)
{
    const char *dot = strchr(detailed, '.');

    if (!dot || dot == detailed || dot[1] == '\0' || strchr(dot + 1, '.') != NULL)
        return FALSE;
    *prefix = g_strndup(detailed, (gsize)(dot - detailed));
    *name = dot + 1;
    return TRUE;
}

static AdwTabView *
tab_view(GtkWidget *window)
{
    GtkWidget *widget;

    if (!window || !load_adw())
        return NULL;
    widget = find_typed(window, adw.get_type());
    return (AdwTabView *)widget;
}

static int
tab_position(GtkWidget *window, int *n_pages)
{
    AdwTabView *view = tab_view(window);
    AdwTabPage *page;

    *n_pages = 0;
    if (!view)
        return -1;
    *n_pages = adw.n_pages(view);
    page = adw.selected(view);
    if (!page)
        return -1;
    return adw.position(view, page);
}

static gboolean
select_tab(GtkWidget *window, gboolean next)
{
    AdwTabView *view = tab_view(window);

    if (!view)
        return FALSE;
    return next ? adw.next(view) : adw.previous(view);
}

static gboolean
action_enabled(GtkWidget *window, const char *detailed)
{
    char *prefix = NULL;
    const char *name = NULL;
    GActionGroup *group;
    gboolean enabled = FALSE;
    int n_pages = 0;
    int position;

    if (!window || !detailed)
        return FALSE;
    if (g_str_equal(detailed, "folio.previous-tab") || g_str_equal(detailed, "folio.next-tab")) {
        position = tab_position(window, &n_pages);
        if (position < 0)
            return FALSE;
        return g_str_equal(detailed, "folio.previous-tab") ? position > 0 : position + 1 < n_pages;
    }
    if (g_str_has_prefix(detailed, "toolbar."))
        return find_class_action(window, detailed) != NULL;
    if (!split_action(detailed, &prefix, &name))
        return FALSE;
    group = group_for(window, prefix);
    if (group)
        enabled = g_action_group_get_action_enabled(group, name);
    g_free(prefix);
    return enabled;
}

static gboolean
activate(GtkWidget *window, const char *detailed, GVariant *target)
{
    char *prefix = NULL;
    const char *name = NULL;
    GActionGroup *group;
    GVariant *state;
    GtkWidget *toolbar;

    if (!window || !detailed)
        return FALSE;
    if (g_str_equal(detailed, "folio.previous-tab"))
        return select_tab(window, FALSE);
    if (g_str_equal(detailed, "folio.next-tab"))
        return select_tab(window, TRUE);
    if (g_str_has_prefix(detailed, "toolbar.")) {
        toolbar = find_class_action(window, detailed);
        return toolbar && gtk_widget_activate_action_variant(toolbar, detailed, target);
    }
    if (!split_action(detailed, &prefix, &name))
        return FALSE;
    group = group_for(window, prefix);
    g_free(prefix);
    if (!group || !g_action_group_has_action(group, name) || !g_action_group_get_action_enabled(group, name))
        return FALSE;
    state = g_action_group_get_action_state(group, name);
    if (target && state) {
        g_variant_unref(state);
        g_action_group_change_action_state(group, name, target);
    } else {
        if (state)
            g_variant_unref(state);
        g_action_group_activate_action(group, name, target);
    }
    return TRUE;
}

static void
handle_method(GDBusConnection *connection,
              const char *sender,
              const char *object_path,
              const char *interface_name,
              const char *method_name,
              GVariant *parameters,
              GDBusMethodInvocation *invocation,
              gpointer user_data)
{
    GtkWidget *window = active_window();

    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)user_data;

    if (g_str_equal(method_name, "Activate")) {
        const char *action = NULL;
        GVariantIter *targets = NULL;
        GVariant *target = NULL;
        gboolean ok;

        /* An empty array is no target. D-Bus has no empty tuple, so the
         * panel cannot send a unit variant for actions that take nothing. */
        g_variant_get(parameters, "(&sav)", &action, &targets);
        g_variant_iter_next(targets, "v", &target);
        g_variant_iter_free(targets);
        ok = activate(window, action, target);
        if (target)
            g_variant_unref(target);
        g_dbus_method_invocation_return_value(invocation, g_variant_new("(b)", ok));
        return;
    }

    if (g_str_equal(method_name, "Enabled")) {
        GVariantIter *iter = NULL;
        const char *action = NULL;
        GVariantBuilder builder;
        GVariant *tuple[1];

        g_variant_builder_init(&builder, G_VARIANT_TYPE("ab"));
        g_variant_get(parameters, "(as)", &iter);
        while (g_variant_iter_loop(iter, "&s", &action))
            g_variant_builder_add(&builder, "b", action_enabled(window, action));
        g_variant_iter_free(iter);
        tuple[0] = g_variant_builder_end(&builder);
        g_dbus_method_invocation_return_value(invocation, g_variant_new_tuple(tuple, 1));
        return;
    }

    g_dbus_method_invocation_return_error_literal(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD, "unknown method");
}

static const GDBusInterfaceVTable vtable = {
    handle_method,
    NULL,
    NULL,
};

static void
on_name_acquired(GDBusConnection *connection, const char *name, gpointer user_data)
{
    GError *error = NULL;

    (void)name;
    (void)user_data;
    registration = g_dbus_connection_register_object(
        connection, FOLIO_OBJECT_PATH, info->interfaces[0], &vtable, NULL, NULL, &error);
    if (error) {
        g_warning("folio files: %s", error->message);
        g_error_free(error);
    }
}

static void
on_name_lost(GDBusConnection *connection, const char *name, gpointer user_data)
{
    (void)name;
    (void)user_data;
    if (connection && registration) {
        g_dbus_connection_unregister_object(connection, registration);
        registration = 0;
    }
}

G_MODULE_EXPORT void
nautilus_module_initialize(GTypeModule *module)
{
    GError *error = NULL;

    (void)module;
    info = g_dbus_node_info_new_for_xml(introspection, &error);
    if (!info) {
        g_warning("folio files: %s", error->message);
        g_error_free(error);
        return;
    }
    name_owner = g_bus_own_name(
        G_BUS_TYPE_SESSION,
        FOLIO_BUS_NAME,
        G_BUS_NAME_OWNER_FLAGS_NONE,
        NULL,
        on_name_acquired,
        on_name_lost,
        NULL,
        NULL);
}

G_MODULE_EXPORT void
nautilus_module_shutdown(void)
{
    if (name_owner) {
        g_bus_unown_name(name_owner);
        name_owner = 0;
    }
    g_clear_pointer(&info, g_dbus_node_info_unref);
}

G_MODULE_EXPORT void
nautilus_module_list_types(const GType **types, int *n_types)
{
    *types = NULL;
    *n_types = 0;
}

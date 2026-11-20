/* justOS's GTK module: what GTK 3 programs need on justOS where no setting
 * can do it. GTK loads it into every GTK 3 program, because
 * /etc/gtk-3.0/settings.ini names it in gtk-modules.
 *
 * It takes the "Trash" entry out of the sidebar of the file chooser. GTK
 * shows it in programs that accept places other than local folders (the
 * file chooser button of gtk3-widget-factory, for one), and opens
 * trash:///, which only GVfs serves, through D-Bus: justOS has neither, so
 * a click on it only said "Operation not supported". justOS's own Cestino
 * is in the Files app. No setting hides the entry: the sidebar shows it
 * unless the program asks for local files only, or its show-trash property
 * is off, so the module turns that property off for every places sidebar
 * when it is shown (the file chooser only ever turns it off itself, in a
 * save dialog, so it stays off).
 *
 * The image build compiles it (Makefile, "root") and links it to the
 * tree's GTK, so that the module finds GTK's functions however the program
 * loaded GTK. There are no GLib or GTK headers in the build: the few
 * functions used are declared below as GLib and GTK 3 define them, and GTK
 * 3 keeps its ABI.
 */

typedef int gboolean;
typedef unsigned int guint;
typedef unsigned long gulong;
typedef unsigned long GType;
typedef void *gpointer;
/* Only pointers to these are used. */
typedef struct _GValue GValue;
typedef struct _GSignalInvocationHint GSignalInvocationHint;
typedef gboolean (*GSignalEmissionHook)(GSignalInvocationHint *hint, guint n_values, const GValue *values,
                                        gpointer data);

extern GType gtk_widget_get_type(void);
extern GType gtk_places_sidebar_get_type(void);
extern void gtk_places_sidebar_set_show_trash(gpointer sidebar, gboolean show_trash);
extern guint g_signal_lookup(const char *name, GType itype);
extern gulong g_signal_add_emission_hook(guint signal_id, guint detail, GSignalEmissionHook hook, gpointer data,
                                         void (*destroy)(gpointer));
extern gpointer g_value_get_object(const GValue *value);
extern gboolean g_type_check_instance_is_a(gpointer instance, GType type);

/* Runs whenever a widget is mapped (shown on the screen); the first value
 * is the widget. */
static gboolean hide_trash(GSignalInvocationHint *hint, guint n_values, const GValue *values, gpointer data) {
    (void)hint;
    (void)data;
    gpointer widget = n_values > 0 ? g_value_get_object(values) : 0;
    if (widget && g_type_check_instance_is_a(widget, gtk_places_sidebar_get_type()))
        gtk_places_sidebar_set_show_trash(widget, 0);
    /* The hook stays for the next widget. */
    return 1;
}

void gtk_module_init(int *argc, char ***argv) {
    (void)argc;
    (void)argv;
    g_signal_add_emission_hook(g_signal_lookup("map", gtk_widget_get_type()), 0, hide_trash, 0, 0);
}

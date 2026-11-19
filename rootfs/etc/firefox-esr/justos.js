// justOS: a file downloaded in Firefox opens as a double click in the Files
// app does, through the system's default handler (justos-open): no "Choose
// Application" window. Programs and installers still get Firefox's "Open
// Executable File?" question once, before they run. Downloads go to the
// folder the Files app shows as "Scaricati".
pref("browser.download.always_ask_before_handling_new_types", false);
pref("browser.download.useDownloadDir", true);
pref("browser.download.folderList", 2);
pref("browser.download.dir", "/home/Scaricati");

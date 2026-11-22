// What the programs in a Wine bottle add to a file's right-click menu, which Wine's own
// file menu leaves out: Wine does not load the handlers programs register for it.
//
//   nekoland-shellmenu.exe FILE            lists the items, a line each: its submenus' names
//                                          and its own, tabs between, as UTF-8
//   nekoland-shellmenu.exe FILE ITEM...    does the item named so
//
// Built with MinGW (see install.sh) and run by nekoland-settings with the bottle's Wine.
// shortcut: 64-bit handlers only, as the program is; a 32-bit one too when a bottle needs it.

#define COBJMACROS
#include <fcntl.h>
#include <io.h>
#include <shlobj.h>
#include <stdio.h>
#include <windows.h>

#define FIRST 1
#define LAST 0x7fff
#define MOST 32
#define LABEL 256
#define DEPTH 2

// Each handler asked, with the first of the numbers its items were given.
static IContextMenu *menus[MOST];
static UINT firsts[MOST];
static CLSID asked[MOST];
static int count;
static UINT next = FIRST;

// The handlers a class of file has registered add their items to `popup`.
static void add_handlers(const WCHAR *class, IDataObject *data, LPCITEMIDLIST folder, HMENU popup) {
    WCHAR path[512], name[LABEL];
    HKEY handlers, of_class = NULL;
    DWORD size;
    swprintf(path, 512, L"%ls\\shellex\\ContextMenuHandlers", class);
    if (RegOpenKeyExW(HKEY_CLASSES_ROOT, path, 0, KEY_READ, &handlers) != ERROR_SUCCESS) return;
    RegOpenKeyExW(HKEY_CLASSES_ROOT, class, 0, KEY_READ, &of_class);
    for (DWORD at = 0; size = LABEL, RegEnumKeyExW(handlers, at, name, &size, NULL, NULL, NULL, NULL) == ERROR_SUCCESS; at++) {
        // The handler's id is the key's value, or its name.
        WCHAR value[LABEL];
        LONG length = sizeof value;
        CLSID id;
        BOOL known = FALSE;
        IShellExtInit *handler;
        IContextMenu *menu = NULL;
        if (RegQueryValueW(handlers, name, value, &length) != ERROR_SUCCESS || FAILED(CLSIDFromString(value, &id))) {
            if (FAILED(CLSIDFromString(name, &id))) continue;
        }
        // One registered for files and for folders both is asked once.
        for (int i = 0; i < count; i++) known = known || IsEqualCLSID(&asked[i], &id);
        if (known || count == MOST) continue;
        if (FAILED(CoCreateInstance(&id, NULL, CLSCTX_INPROC_SERVER, &IID_IShellExtInit, (void **)&handler))) continue;
        if (SUCCEEDED(IShellExtInit_Initialize(handler, folder, data, of_class)) &&
            SUCCEEDED(IShellExtInit_QueryInterface(handler, &IID_IContextMenu, (void **)&menu))) {
            HRESULT added = IContextMenu_QueryContextMenu(menu, popup, GetMenuItemCount(popup), next, LAST, CMF_NORMAL);
            if (SUCCEEDED(added)) {
                menus[count] = menu;
                firsts[count] = next;
                asked[count++] = id;
                next += HRESULT_CODE(added);
            } else {
                IContextMenu_Release(menu);
            }
        }
        IShellExtInit_Release(handler);
    }
    if (of_class) RegCloseKey(of_class);
    RegCloseKey(handlers);
}

// The items of a menu and its submenus: each printed by the names that lead to it and its
// own, or the one named `wanted` done. Whether that one was found.
static BOOL walk(HMENU popup, const WCHAR *above, int depth, const WCHAR *wanted) {
    int items = GetMenuItemCount(popup);
    for (int at = 0; at < items; at++) {
        WCHAR label[LABEL], path[4 * LABEL], *from, *to;
        MENUITEMINFOW info = {sizeof info};
        info.fMask = MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_FTYPE;
        info.dwTypeData = label;
        info.cch = LABEL - 1;
        if (!GetMenuItemInfoW(popup, at, TRUE, &info) || (info.fType & MFT_SEPARATOR) || (info.fState & MFS_GRAYED)) continue;
        // Without the letter underlined for the keyboard, and the keys said after a tab.
        for (from = to = label; *from && *from != L'\t'; from++) {
            if (from[0] == L'(' && from[1] == L'&' && from[2] && from[3] == L')') from += 3;
            else if (*from != L'&') *to++ = *from;
        }
        while (to > label && to[-1] == L' ') to--;
        *to = 0;
        if (!*label) continue;
        swprintf(path, 4 * LABEL, above ? L"%ls\t%ls" : L"%ls%ls", above ? above : L"", label);
        if (info.hSubMenu) {
            if (depth < DEPTH && walk(info.hSubMenu, path, depth + 1, wanted)) return TRUE;
        } else if (info.wID >= FIRST && info.wID < next) {
            if (!wanted) {
                char line[12 * LABEL];
                int length = WideCharToMultiByte(CP_UTF8, 0, path, -1, line, sizeof line - 1, NULL, NULL);
                if (length > 0) {
                    line[length - 1] = '\n';
                    fwrite(line, 1, length, stdout);
                }
            } else if (!wcscmp(path, wanted)) {
                // The handler whose numbers this one is among, and its place among them.
                int of = count - 1;
                CMINVOKECOMMANDINFO command = {sizeof command};
                while (of > 0 && firsts[of] > info.wID) of--;
                command.lpVerb = MAKEINTRESOURCEA(info.wID - firsts[of]);
                command.nShow = SW_SHOWNORMAL;
                IContextMenu_InvokeCommand(menus[of], &command);
                return TRUE;
            }
        }
    }
    return FALSE;
}

int wmain(int argc, WCHAR **argv) {
    WCHAR wanted[4 * LABEL] = L"", *extension, kind[LABEL];
    LPITEMIDLIST whole, folder;
    LPCITEMIDLIST file;
    IShellFolder *parent;
    IDataObject *data;
    HMENU popup;
    LONG length = sizeof kind;
    DWORD attributes;
    if (argc < 2) return 2;
    for (int i = 2; i < argc; i++) {
        if (i > 2) wcscat(wanted, L"\t");
        wcsncat(wanted, argv[i], LABEL);
    }
    _setmode(_fileno(stdout), _O_BINARY);
    if (FAILED(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED))) return 1;
    if (FAILED(SHParseDisplayName(argv[1], NULL, &whole, 0, NULL))) return 1;
    if (FAILED(SHBindToParent(whole, &IID_IShellFolder, (void **)&parent, &file))) return 1;
    if (FAILED(IShellFolder_GetUIObjectOf(parent, NULL, 1, &file, &IID_IDataObject, NULL, (void **)&data))) return 1;
    folder = ILClone(whole);
    ILRemoveLastID(folder);
    popup = CreatePopupMenu();

    attributes = GetFileAttributesW(argv[1]);
    if (attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY)) {
        add_handlers(L"Directory", data, folder, popup);
        add_handlers(L"Folder", data, folder, popup);
    } else {
        add_handlers(L"*", data, folder, popup);
        // The file's extension, and the kind of file the extension is said to be.
        extension = wcsrchr(argv[1], L'.');
        if (extension && !wcschr(extension, L'\\')) {
            add_handlers(extension, data, folder, popup);
            if (RegQueryValueW(HKEY_CLASSES_ROOT, extension, kind, &length) == ERROR_SUCCESS && *kind) {
                add_handlers(kind, data, folder, popup);
            }
        }
    }
    add_handlers(L"AllFilesystemObjects", data, folder, popup);

    if (!walk(popup, NULL, 0, argc > 2 ? wanted : NULL) && argc > 2) return 3;
    fflush(stdout);
    return 0;
}

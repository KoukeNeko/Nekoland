# Nekoland's agent in Windows. The one connection Nekoland keeps to Windows runs it as its
# RemoteApp program, from the folder Nekoland shares; it lives as long as that folder is
# there, which is as long as the connection.
#
# It does what Nekoland asks in requests\*.txt, a line of tab-separated fields each:
#   run <program> [<file>]   starts a program, with the file open in it
#   find                     lists the Start menu's programs into found\apps.tsv, the
#                            Store's among them, as shell:AppsFolder\<their name there>
#   menu <file> <name>       lists what programs add to the file's right-click menu into
#                            menus\<name>.tsv, a line an item: its submenus' names and its own
#   verb <file> <item>...    does one of them
# and keeps windows.tsv, written anew at least every 10 seconds, a line for each window
# shown: its handle, which FreeRDP names its window after, its program's path, how much of
# its top is a title bar of Windows', for Nekoland to draw its own over, its width and
# height, and for a program from the Store what it is started by, which unlike its path
# stays the same from one update to the next.

$share = '\\tsclient\nekoland'

# One agent: the one a connection before this left behind, should it be there still, goes.
Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" |
    Where-Object { $_.CommandLine -match 'agent\.ps1' -and $_.ProcessId -ne $PID } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

public static class Neko {
    public delegate bool EnumProc(IntPtr window, IntPtr data);
    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct POINT { public int X, Y; }
    public struct TITLEBARINFO {
        public int Size;
        public RECT Bar;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 6)] public int[] States;
    }

    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc each, IntPtr data);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] static extern IntPtr GetLong(IntPtr window, int index);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] static extern bool GetTitleBarInfo(IntPtr window, ref TITLEBARINFO info);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr window, ref POINT point);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string kind, string title);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint process);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern int GetApplicationUserModelId(IntPtr process, ref uint length, StringBuilder id);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr window, int attribute, out int value, int size);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr window, int attribute, out RECT value, int size);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint PrivateExtractIcons(string file, int index, int cx, int cy, IntPtr[] icons, uint[] ids, uint count, uint flags);

    [ComImport, Guid("43826d1e-e718-42ee-bc55-a1e261c37bfe"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IShellItem {
        [PreserveSig] int BindToHandler(IntPtr context, ref Guid handler, ref Guid id, [MarshalAs(UnmanagedType.IUnknown)] out object made);
    }
    [ComImport, Guid("000214e4-0000-0000-c000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IContextMenu {
        [PreserveSig] int QueryContextMenu(IntPtr menu, uint at, uint first, uint last, uint flags);
        [PreserveSig] int InvokeCommand(ref CMINVOKECOMMANDINFO info);
        [PreserveSig] int GetCommandString(UIntPtr id, uint kind, IntPtr reserved, [MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, uint size);
    }
    public struct CMINVOKECOMMANDINFO {
        public int Size; public uint Mask; public IntPtr Window, Verb, Parameters, Directory;
        public int Show; public uint HotKey; public IntPtr Icon;
    }
    public struct MENUITEMINFO {
        public int Size; public uint Mask, Type, State, Id;
        public IntPtr SubMenu, Checked, Unchecked, Data, Text;
        public uint Length; public IntPtr Bitmap;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr context, ref Guid id, out IShellItem item);
    [DllImport("user32.dll")] static extern IntPtr CreatePopupMenu();
    [DllImport("user32.dll")] static extern bool DestroyMenu(IntPtr menu);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr menu);
    [DllImport("user32.dll", EntryPoint = "GetMenuItemInfoW")] static extern bool GetMenuItemInfo(IntPtr menu, uint item, bool byPosition, ref MENUITEMINFO info);

    const uint FIRST = 1, LAST = 0x7FFF, GCS_VERBW = 4;
    const uint MIIM_STATE = 1, MIIM_ID = 2, MIIM_SUBMENU = 4, MIIM_STRING = 0x40, MIIM_FTYPE = 0x100;
    const uint MFT_SEPARATOR = 0x800, MFS_GRAYED = 3;
    // What Windows' own file manager puts in the menu, which Nekoland's has its own of.
    static readonly string[] OWN = {
        "open", "opennew", "opennewwindow", "opennewtab", "opennewprocess", "openas", "explore", "find",
        "cut", "copy", "paste", "pastelink", "delete", "rename", "properties", "link", "copyaspath",
        "pintohome", "pintohomefile", "pintostartscreen", "startpin", "taskbarpin", "sendto", "share",
        "windows.share", "windows.modernshare", "previousversions", "includeinlibrary"
    };

    // The items of a menu and of its submenus, each by the names that lead to it and its own,
    // tabs between, with the number the menu knows it by.
    static void Items(IContextMenu menu, IntPtr popup, string above, int depth, List<KeyValuePair<string, uint>> items) {
        int count = GetMenuItemCount(popup);
        for (uint at = 0; at < count; at++) {
            var info = new MENUITEMINFO();
            info.Size = Marshal.SizeOf(typeof(MENUITEMINFO));
            info.Mask = MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_FTYPE;
            info.Text = Marshal.AllocHGlobal(1024);
            info.Length = 511;
            bool got = GetMenuItemInfo(popup, at, true, ref info);
            string label = got ? Marshal.PtrToStringUni(info.Text) : null;
            Marshal.FreeHGlobal(info.Text);
            if (String.IsNullOrEmpty(label) || (info.Type & MFT_SEPARATOR) != 0 || (info.State & MFS_GRAYED) != 0) continue;
            // Without the letter underlined for the keyboard, and the keys said after a tab.
            label = Regex.Replace(label.Split('\t')[0], @"\(&.\)|&", "").Trim();
            bool known = info.Id >= FIRST && info.Id <= LAST;
            string verb = "";
            if (known) {
                var name = new StringBuilder(256);
                try {
                    if (menu.GetCommandString((UIntPtr)(info.Id - FIRST), GCS_VERBW, IntPtr.Zero, name, 256) == 0) verb = name.ToString().ToLowerInvariant();
                } catch (Exception) { }
            }
            if (label.Length == 0 || Array.IndexOf(OWN, verb) >= 0) continue;
            string path = above == null ? label : above + "\t" + label;
            if (info.SubMenu != IntPtr.Zero) {
                if (depth >= 2) continue;
                // What fills a submenu only as it opens is left unfilled, and so not listed.
                Items(menu, info.SubMenu, path, depth + 1, items);
            } else if (known) {
                items.Add(new KeyValuePair<string, uint>(path, info.Id));
            }
        }
    }

    // What programs add to a file's right-click menu, listed, or the one of them `chosen`
    // names done. In a thread of its own, as the menu's parts are made for one that is the
    // only one to call them.
    public static List<string> Menu(string path, string[] chosen) {
        var said = new List<string>();
        Exception failed = null;
        string step = "the file";
        var thread = new Thread(() => {
            IntPtr popup = CreatePopupMenu();
            try {
                Guid item = typeof(IShellItem).GUID, ui = new Guid("3981e225-f559-11d3-8e3a-00c04f6837d5"), id = typeof(IContextMenu).GUID;
                IShellItem file;
                SHCreateItemFromParsingName(path, IntPtr.Zero, ref item, out file);
                object made;
                step = "its menu";
                Marshal.ThrowExceptionForHR(file.BindToHandler(IntPtr.Zero, ref ui, ref id, out made));
                var menu = (IContextMenu)made;
                step = "the menu's items";
                int asked = menu.QueryContextMenu(popup, 0, FIRST, LAST, 0);
                if (asked < 0) Marshal.ThrowExceptionForHR(asked);
                var items = new List<KeyValuePair<string, uint>>();
                step = "the items' names";
                Items(menu, popup, null, 0, items);
                if (chosen == null) {
                    foreach (var one in items) said.Add(one.Key);
                    return;
                }
                string wanted = String.Join("\t", chosen);
                int at = items.FindIndex(one => one.Key == wanted);
                if (at < 0) throw new Exception(wanted + ": not in the menu");
                var info = new CMINVOKECOMMANDINFO();
                info.Size = Marshal.SizeOf(typeof(CMINVOKECOMMANDINFO));
                info.Verb = (IntPtr)(items[at].Value - FIRST);
                info.Show = 1;
                step = "doing " + wanted;
                Marshal.ThrowExceptionForHR(menu.InvokeCommand(ref info));
            } catch (Exception err) {
                failed = err;
            } finally {
                DestroyMenu(popup);
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
        // A program's part of the menu that never answers is not to keep the rest waiting.
        if (!thread.Join(20000)) throw new Exception("no answer at " + step);
        if (failed != null) throw failed;
        return said;
    }

    // What a process from the Store is known by there, or nothing for any other.
    public static string Model(uint process) {
        IntPtr handle = OpenProcess(0x1000, false, process);
        if (handle == IntPtr.Zero) return "";
        uint length = 256;
        var id = new StringBuilder(256);
        int failed = GetApplicationUserModelId(handle, ref length, id);
        CloseHandle(handle);
        return failed == 0 ? id.ToString() : "";
    }

    const int GWL_STYLE = -16, GWL_EXSTYLE = -20, DWMWA_CLOAKED = 14, DWMWA_EXTENDED_FRAME_BOUNDS = 9;
    const long WS_CAPTION = 0xC00000, WS_EX_TOOLWINDOW = 0x80;

    // The windows shown, as "handle process title width height": how much of the window's
    // top is a title bar Windows draws, for Nekoland to draw its own over, or 0 for a window
    // that draws one itself, in what is its own to draw in; and how large what is seen of
    // the window is, which FreeRDP's is larger than, by the margins Windows resizes it at.
    public static List<string> Windows() {
        var shown = new List<string>();
        EnumWindows((window, data) => {
            int cloaked;
            DwmGetWindowAttribute(window, DWMWA_CLOAKED, out cloaked, 4);
            long extended = GetLong(window, GWL_EXSTYLE).ToInt64();
            if (!IsWindowVisible(window) || cloaked != 0 || (extended & WS_EX_TOOLWINDOW) != 0) return true;
            RECT whole;
            GetWindowRect(window, out whole);
            if (whole.Right - whole.Left < 2 || whole.Bottom - whole.Top < 2) return true;
            var inside = new POINT();
            ClientToScreen(window, ref inside);
            long style = GetLong(window, GWL_STYLE).ToInt64();
            // A program that draws its own title bar has its own part of the window start
            // above where Windows' would end.
            var bar = new TITLEBARINFO();
            bar.Size = Marshal.SizeOf(typeof(TITLEBARINFO));
            int title = 0;
            if ((style & WS_CAPTION) == WS_CAPTION && GetTitleBarInfo(window, ref bar) && inside.Y >= bar.Bar.Bottom - 1)
                title = bar.Bar.Bottom - whole.Top;
            uint process;
            GetWindowThreadProcessId(window, out process);
            // Windows frames some of the Store's programs in a process of its own: the
            // program is the one whose window is inside.
            IntPtr inside_frame = FindWindowEx(window, IntPtr.Zero, "Windows.UI.Core.CoreWindow", null);
            if (inside_frame != IntPtr.Zero) GetWindowThreadProcessId(inside_frame, out process);
            // What is seen of the window: without the margins around it that Windows resizes
            // it at, which are the window's too.
            RECT seen;
            if (DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS, out seen, 16) != 0) seen = whole;
            shown.Add(String.Format("{0:X8}\t{1}\t{2}\t{3}\t{4}", window.ToInt64(), process, title > 20 ? title : 0,
                seen.Right - seen.Left, seen.Bottom - seen.Top));
            return true;
        }, IntPtr.Zero);
        return shown;
    }
}
'@

# Windows signed in on its own screen at startup would ask each connection whether to take
# that session over.
reg add 'HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' /v AutoAdminLogon /t REG_SZ /d 0 /f | Out-Null

# Windows sends a remote session at most 30 frames a second; 15 here raises that past 60,
# from the next session Windows starts.
reg add 'HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations' /v DWMFRAMEINTERVAL /t REG_DWORD /d 15 /f | Out-Null

# Lists the Start menu's programs, one a line: name, path, the number its icon is saved
# under, and the extensions Windows opens with it. The list is written whole and then
# named, so that it is never read half written.
function Find-Apps {
    $out = "$share\found"
    New-Item -ItemType Directory -Force $out | Out-Null
    $types = @{}
    cmd /c assoc | ForEach-Object {
        $ext, $type = $_ -split '=', 2
        if ($type) { $types[$type] = @($types[$type]) + $ext }
    }
    $opens = @{}
    cmd /c ftype | ForEach-Object {
        $type, $command = $_ -split '=', 2
        if ($command -match '^"?([^"]+?\.exe)' -and $types[$type]) {
            $exe = [Environment]::ExpandEnvironmentVariables($matches[1]).ToLower()
            $opens[$exe] = @($opens[$exe]) + $types[$type]
        }
    }
    $shell = New-Object -ComObject WScript.Shell
    $menus = [Environment]::GetFolderPath('CommonPrograms'), [Environment]::GetFolderPath('Programs')
    $seen = @{}
    $lines = Get-ChildItem $menus -Recurse -Filter *.lnk | ForEach-Object {
        if ($_.FullName -match 'Administrative Tools|System Tools|Accessibility|Startup') { return }
        $link = $shell.CreateShortcut($_.FullName)
        $target = $link.TargetPath
        if ($target -notmatch '\.exe$' -or $_.BaseName -match 'uninstall' -or $seen[$target.ToLower()]) { return }
        if (-not (Test-Path -LiteralPath $target)) { return }
        $seen[$target.ToLower()] = $true
        $number = $seen.Count
        $icon, $index = $link.IconLocation -split ',', 2
        $icon = [Environment]::ExpandEnvironmentVariables($icon)
        if (-not $icon) { $icon = $target; $index = 0 }
        $handles = New-Object IntPtr[] 1
        if ([Neko]::PrivateExtractIcons($icon, [int]$index, 256, 256, $handles, $null, 1, 0) -ge 1 -and $handles[0] -ne [IntPtr]::Zero) {
            [System.Drawing.Icon]::FromHandle($handles[0]).ToBitmap().Save("$out\$number.png", [System.Drawing.Imaging.ImageFormat]::Png)
        }
        $extensions = ($opens[$target.ToLower()] | Select-Object -Unique) -join ' '
        "$($_.BaseName)`t$target`t$number`t$extensions"
    }
    # The Store's, which the Start menu has no shortcuts for: each with the largest picture
    # its package has of it.
    $packages = @{}
    Get-AppxPackage | ForEach-Object { $packages[$_.PackageFamilyName] = $_ }
    $number = $seen.Count
    $lines = @($lines) + @(Get-StartApps | Where-Object { $_.AppID -match '^[^\\]+!' } | ForEach-Object {
        $number++
        $family, $id = $_.AppID -split '!', 2
        $package = $packages[$family]
        if ($package) {
            try {
                $app = (Get-AppxPackageManifest $package).Package.Applications.Application | Where-Object Id -eq $id
                $logo = $app.VisualElements.Square44x44Logo
                $named = Join-Path $package.InstallLocation $logo
                $largest = Get-ChildItem (Split-Path $named) -Filter "$([IO.Path]::GetFileNameWithoutExtension($named))*.png" |
                    Where-Object Name -notmatch 'contrast' | Sort-Object Length -Descending | Select-Object -First 1
                if ($largest) { Copy-Item -LiteralPath $largest.FullName "$out\$number.png" }
            } catch {
                [IO.File]::AppendAllText("$share\agent.log", "icon of $($_.AppID): $_`r`n")
            }
        }
        "$($_.Name)`tshell:AppsFolder\$($_.AppID)`t$number`t"
    })
    [IO.File]::WriteAllLines("$out\apps.new", @($lines))
    Move-Item -Force "$out\apps.new" "$out\apps.tsv"
}

$paths = @{}
$models = @{}
$last = $null
$written = [Diagnostics.Stopwatch]::StartNew()
while (Test-Path $share) {
    foreach ($request in @(Get-ChildItem "$share\requests" -Filter *.txt -ErrorAction SilentlyContinue | Sort-Object Name)) {
        $verb, $program, $file = (Get-Content -LiteralPath $request.FullName -Encoding UTF8 -TotalCount 1) -split "`t"
        Remove-Item -LiteralPath $request.FullName
        try {
            if ($verb -eq 'find') { Find-Apps }
            # One from the Store through Explorer, which does not wait for it: started from
            # here, a program busy with a window of its own kept the agent waiting with it.
            elseif ($verb -eq 'run' -and $program -like 'shell:*') { Start-Process explorer.exe -ArgumentList "`"$program`"" }
            elseif ($verb -eq 'run' -and $file) { Start-Process -FilePath $program -ArgumentList "`"$file`"" }
            elseif ($verb -eq 'run') { Start-Process -FilePath $program }
            elseif ($verb -eq 'menu') {
                New-Item -ItemType Directory -Force "$share\menus" | Out-Null
                # Not Windows' own submenu to send the file somewhere, which gives no name to know
                # it by: it is the one with what the folder of those places holds.
                $places = @((New-Object -ComObject Shell.Application).NameSpace(9).Items() | ForEach-Object Name)
                $items = @([Neko]::Menu($program, $null))
                $sending = @($items | ForEach-Object { $above, $name = $_ -split "`t", 2; if ($places -contains $name) { $above } })
                [IO.File]::WriteAllLines("$share\menus\$file.new", [string[]]@($items | Where-Object { $sending -notcontains ($_ -split "`t")[0] }))
                Move-Item -Force "$share\menus\$file.new" "$share\menus\$file.tsv"
            }
            elseif ($verb -eq 'verb') { [Neko]::Menu($program, [string[]]@($file)) | Out-Null }
        } catch {
            [IO.File]::AppendAllText("$share\agent.log", "$verb $program`: $_`r`n")
        }
    }
    $lines = foreach ($window in [Neko]::Windows()) {
        $handle, $process, $title, $width, $height = $window -split "`t"
        if (-not $paths.ContainsKey($process)) {
            $paths[$process] = (Get-Process -Id $process -ErrorAction SilentlyContinue).Path
            $model = [Neko]::Model($process)
            $models[$process] = if ($model) { "shell:AppsFolder\$model" }
        }
        "$handle`t$($paths[$process])`t$title`t$width`t$height`t$($models[$process])"
    }
    $now = @($lines) -join "`n"
    # Written anew every 10 seconds all the same, for Nekoland to know the agent is there.
    if ($now -ne $last -or $written.Elapsed.TotalSeconds -ge 10 -or -not (Test-Path "$share\windows.tsv")) {
        [IO.File]::WriteAllText("$share\windows.new", "$now`n")
        Move-Item -Force "$share\windows.new" "$share\windows.tsv"
        $last = $now
        $written.Restart()
    }
    Start-Sleep -Milliseconds 250
}

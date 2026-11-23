using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.ServiceProcess;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32;

namespace TetherSetup;

/// <summary>
/// Installs Tether into Program Files and registers Tether Camera (the virtual camera DLL) with Windows. Only files
/// named in Tether's file list are ever replaced or removed. Settings in %AppData%\Tether are never touched.
/// </summary>
internal static class Installer
{
    public const string ManifestName = "tether-files.txt";
    public const string CameraDll = "tether_vcam.dll";
    private const string UninstallKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\Tether";

    // Windows' camera service runs as a service account and can only load the DLL from a folder it can read, so the
    // folder is fixed.
    public static string DefaultFolder =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Tether");

    public static string Version
    {
        get
        {
            var v = Assembly.GetExecutingAssembly().GetName().Version;
            return $"{v.Major}.{v.Minor}.{v.Build}";
        }
    }

    public static bool HasPayload => Assembly.GetExecutingAssembly().GetManifestResourceNames().Contains("payload.zip");

    public static bool IsInstalled(string folder) => File.Exists(Path.Combine(folder, ManifestName));

    public static bool RunsFromInstallFolder() =>
        string.Equals(Path.GetDirectoryName(Application.ExecutablePath)?.TrimEnd('\\'), DefaultFolder.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase);

    /// <summary>Virtual cameras came with Windows 11 (build 22000).</summary>
    public static bool IsWindows11()
    {
        using var key = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        return int.TryParse(key?.GetValue("CurrentBuildNumber") as string, out int build) && build >= 22000;
    }

    public static bool IsRunning()
    {
        var copies = TetherProcesses();
        copies.ForEach(p => p.Dispose());
        return copies.Count > 0;
    }

    /// <summary>Running Tether apps (any folder: two can't share the phone and the camera), never another "tether.exe".</summary>
    private static List<Process> TetherProcesses()
    {
        var result = new List<Process>();
        foreach (var proc in Process.GetProcessesByName("tether"))
        {
            string product = null;
            try { product = proc.MainModule?.FileVersionInfo.ProductName; } catch { }
            if (product == "Tether") result.Add(proc);
            else proc.Dispose();
        }
        return result;
    }

    public static void Install(SetupOptions options, Action<double, string> report)
    {
        if (!HasPayload) throw new InvalidOperationException("This setup doesn't contain Tether. Download the full setup from GitHub.");
        if (!IsWindows11()) throw new InvalidOperationException("Tether Camera needs Windows 11.");
        string folder = options.Folder;

        report?.Invoke(0, "Closing Tether");
        CloseTether(folder);

        // Only an update has a Tether Camera to release; a fresh install leaves Windows' camera service alone.
        string oldDll = Path.Combine(folder, CameraDll);
        string dev = Path.Combine(folder, "dev"); // a development copy from before the first release
        if (File.Exists(oldDll) || Directory.Exists(dev))
        {
            report?.Invoke(0.03, "Releasing Tether Camera");
            UnregisterCamera(oldDll);
            if (Directory.Exists(dev)) UnregisterCamera(Path.Combine(dev, CameraDll));
            RestartCameraService();
            if (Directory.Exists(dev)) Retry(() => Directory.Delete(dev, true));
        }

        using (var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream("payload.zip"))
        using (var zip = new ZipArchive(stream, ZipArchiveMode.Read))
        {
            var manifest = zip.GetEntry(ManifestName) ?? throw new InvalidDataException("The setup is damaged: its file list is missing.");
            List<string> newFiles;
            using (var reader = new StreamReader(manifest.Open(), Encoding.UTF8)) newFiles = ParseManifest(reader.ReadToEnd());
            var oldFiles = IsInstalled(folder) ? ParseManifest(File.ReadAllText(Path.Combine(folder, ManifestName))) : new List<string>();

            Directory.CreateDirectory(folder);
            report?.Invoke(0.08, "Removing old files");
            foreach (string rel in oldFiles.Except(newFiles, StringComparer.OrdinalIgnoreCase))
                Retry(() => { string path = Path.Combine(folder, rel); if (File.Exists(path)) File.Delete(path); });

            var entries = zip.Entries.Where(e => e.Name.Length > 0).ToList();
            long total = Math.Max(1, entries.Sum(e => e.Length)), done = 0;
            foreach (var entry in entries)
            {
                string target = Path.Combine(folder, CheckRelative(entry.FullName));
                Directory.CreateDirectory(Path.GetDirectoryName(target));
                Retry(() =>
                {
                    using (var input = entry.Open())
                    using (var output = File.Create(target))
                        input.CopyTo(output);
                });
                done += entry.Length;
                report?.Invoke(0.1 + 0.75 * done / total, "Copying files");
            }
        }
        RemoveEmptyFolders(folder);

        report?.Invoke(0.88, "Adding Tether Camera to Windows");
        RegisterCamera(Path.Combine(folder, CameraDll));

        string exe = Path.Combine(folder, "tether.exe");
        report?.Invoke(0.94, "Adding shortcuts");
        CreateShortcut(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms), "Tether.lnk"), exe);
        string desktop = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory), "Tether.lnk");
        if (options.DesktopShortcut) CreateShortcut(desktop, exe);
        else if (options.Mode != Mode.Update && File.Exists(desktop)) File.Delete(desktop);
        Register(folder, exe);
        report?.Invoke(1, "Done");
    }

    public static void Uninstall(Action<double, string> report)
    {
        string folder = DefaultFolder;
        report?.Invoke(0, "Closing Tether");
        CloseTether(folder);
        report?.Invoke(0.15, "Removing Tether Camera from Windows");
        UnregisterCamera(Path.Combine(folder, CameraDll));
        RestartCameraService();

        report?.Invoke(0.4, "Removing files");
        if (IsInstalled(folder))
        {
            foreach (string rel in ParseManifest(File.ReadAllText(Path.Combine(folder, ManifestName))))
                Retry(() => { string path = Path.Combine(folder, rel); if (File.Exists(path)) File.Delete(path); });
            Retry(() => File.Delete(Path.Combine(folder, ManifestName)));
        }
        if (Directory.Exists(folder))
        {
            RemoveEmptyFolders(folder);
            try { if (!Directory.EnumerateFileSystemEntries(folder).Any()) Directory.Delete(folder); } catch { }
        }

        report?.Invoke(0.85, "Removing shortcuts");
        foreach (var dir in new[] { Environment.SpecialFolder.CommonPrograms, Environment.SpecialFolder.CommonDesktopDirectory })
        {
            string link = Path.Combine(Environment.GetFolderPath(dir), "Tether.lnk");
            try { if (File.Exists(link)) File.Delete(link); } catch { }
        }
        Registry.LocalMachine.DeleteSubKeyTree(UninstallKey, false);
        report?.Invoke(1, "Done");
    }

    /// <summary>Starts Tether as the signed-in user, not as admin (Explorer starts it for us).</summary>
    public static void Launch(string folder)
    {
        string exe = Path.Combine(folder, "tether.exe");
        if (File.Exists(exe)) Process.Start(new ProcessStartInfo("explorer.exe", $"\"{exe}\"") { UseShellExecute = false });
    }

    // ---------- helpers ----------

    private static void CloseTether(string folder)
    {
        // Tether's bundled adb leaves a background server running after Tether quits; it locks adb.exe.
        string ours = Path.GetFullPath(Path.Combine(folder, "adb")) + Path.DirectorySeparatorChar;
        var adbs = new List<Process>();
        foreach (var proc in Process.GetProcessesByName("adb"))
        {
            string path = null;
            try { path = proc.MainModule?.FileName; } catch { }
            if (path != null && path.StartsWith(ours, StringComparison.OrdinalIgnoreCase)) adbs.Add(proc);
            else proc.Dispose();
        }
        foreach (var proc in TetherProcesses().Concat(adbs))
        {
            using (proc)
            {
                try
                {
                    proc.Kill();
                    proc.WaitForExit(5000);
                }
                catch { }
            }
        }
    }

    private static void RegisterCamera(string dll)
    {
        int code = RunHidden("regsvr32.exe", $"/s \"{dll}\"");
        if (code != 0) throw new InvalidOperationException($"Windows didn't accept Tether Camera (regsvr32 error {code}).");
    }

    private static void UnregisterCamera(string dll)
    {
        if (File.Exists(dll)) RunHidden("regsvr32.exe", $"/s /u \"{dll}\"");
    }

    /// <summary>Windows' camera service keeps the camera DLL loaded; restarting it lets us replace the file.</summary>
    private static void RestartCameraService()
    {
        foreach (string name in new[] { "FrameServerMonitor", "FrameServer" })
        {
            try
            {
                using var sc = new ServiceController(name);
                if (sc.Status != ServiceControllerStatus.Stopped)
                {
                    sc.Stop();
                    sc.WaitForStatus(ServiceControllerStatus.Stopped, TimeSpan.FromSeconds(10));
                }
            }
            catch { } // not running or not there: nothing holds the DLL
        }
        // They start again by themselves the next time an app opens a camera.
    }

    private static int RunHidden(string exe, string args)
    {
        using var p = Process.Start(new ProcessStartInfo(exe, args) { UseShellExecute = false, CreateNoWindow = true });
        p.WaitForExit(30000);
        return p.HasExited ? p.ExitCode : -1;
    }

    private static List<string> ParseManifest(string text) =>
        text.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries).Select(l => l.Trim()).Where(l => l.Length > 0).Select(CheckRelative).ToList();

    private static string CheckRelative(string path)
    {
        string rel = path.Replace('/', '\\');
        if (Path.IsPathRooted(rel) || rel.Split('\\').Contains("..")) throw new InvalidDataException($"Unexpected file in the setup: {path}");
        return rel;
    }

    private static void Retry(Action action)
    {
        for (int attempt = 0; ; attempt++)
        {
            try
            {
                action();
                return;
            }
            catch (Exception ex) when ((ex is IOException || ex is UnauthorizedAccessException) && attempt < 40)
            {
                Thread.Sleep(250); // a closing Tether, the camera service or an antivirus scan can hold a file for a moment
            }
        }
    }

    private static void RemoveEmptyFolders(string root)
    {
        foreach (string dir in Directory.GetDirectories(root, "*", SearchOption.AllDirectories).OrderByDescending(d => d.Length))
        {
            try { if (!Directory.EnumerateFileSystemEntries(dir).Any()) Directory.Delete(dir); } catch { }
        }
    }

    private static void Register(string folder, string exe)
    {
        long kb = new DirectoryInfo(folder).GetFiles("*", SearchOption.AllDirectories).Sum(f => f.Length) / 1024;
        using var key = Registry.LocalMachine.CreateSubKey(UninstallKey);
        key.SetValue("DisplayName", "Tether");
        key.SetValue("DisplayVersion", Version);
        key.SetValue("Publisher", "Tether");
        key.SetValue("DisplayIcon", exe);
        key.SetValue("InstallLocation", folder);
        key.SetValue("UninstallString", $"\"{Path.Combine(folder, "uninstall.exe")}\" /uninstall");
        key.SetValue("URLInfoAbout", "https://github.com/slashneck/Tether");
        key.SetValue("NoModify", 1, RegistryValueKind.DWord);
        key.SetValue("NoRepair", 1, RegistryValueKind.DWord);
        key.SetValue("EstimatedSize", (int)Math.Min(int.MaxValue, kb), RegistryValueKind.DWord);
    }

    private static void CreateShortcut(string path, string target)
    {
        var link = (IShellLinkW)new ShellLink();
        link.SetPath(target);
        link.SetWorkingDirectory(Path.GetDirectoryName(target));
        link.SetIconLocation(target, 0);
        link.SetDescription("Tether");
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        ((IPersistFile)link).Save(path, false);
        Marshal.ReleaseComObject(link);
    }

    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLink { }

    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("000214F9-0000-0000-C000-000000000046")]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder file, int max, IntPtr findData, int flags);
        void GetIDList(out IntPtr idList);
        void SetIDList(IntPtr idList);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder name, int max);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string name);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder dir, int max);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string dir);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder args, int max);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string args);
        void GetHotkey(out short hotkey);
        void SetHotkey(short hotkey);
        void GetShowCmd(out int showCmd);
        void SetShowCmd(int showCmd);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int max, out int index);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, int reserved);
        void Resolve(IntPtr hwnd, int flags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string file);
    }
}

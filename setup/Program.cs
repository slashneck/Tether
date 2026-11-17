using System;
using System.Diagnostics;
using System.IO;
using System.Windows.Forms;

namespace TetherSetup;

internal enum Mode { Install, Update, Uninstall }

internal sealed class SetupOptions
{
    public string Folder = Installer.DefaultFolder;
    public bool DesktopShortcut = true;
    public bool Launch = true;
    public bool Silent;
    public Mode Mode = Mode.Install;

    /// <summary>
    /// /update (from Tether's updater: installs right away, then starts Tether again), /uninstall, /silent.
    /// </summary>
    public static SetupOptions Parse(string[] args)
    {
        var options = new SetupOptions();
        foreach (string raw in args)
        {
            string arg = raw.TrimStart('/', '-').ToLowerInvariant();
            if (arg == "update") options.Mode = Mode.Update;
            else if (arg == "uninstall") options.Mode = Mode.Uninstall;
            else if (arg == "silent") { options.Silent = true; options.Launch = false; }
            else if (arg == "nodesktop") options.DesktopShortcut = false;
        }
        // The uninstaller has no app inside; started without arguments it still uninstalls.
        if (!Installer.HasPayload) options.Mode = Mode.Uninstall;
        return options;
    }
}

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        var options = SetupOptions.Parse(args);

        // The uninstaller can't delete itself while it runs from the install folder: run a copy from %TEMP%.
        if (options.Mode == Mode.Uninstall && Installer.RunsFromInstallFolder())
        {
            string copy = Path.Combine(Path.GetTempPath(), $"TetherUninstall-{Guid.NewGuid():N}.exe");
            File.Copy(Application.ExecutablePath, copy, true);
            Process.Start(new ProcessStartInfo(copy, string.Join(" ", args) + " /uninstall") { UseShellExecute = false });
            return 0;
        }

        if (options.Silent)
        {
            try
            {
                if (options.Mode == Mode.Uninstall) Installer.Uninstall(null);
                else Installer.Install(options, null);
                return 0;
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine(ex.Message);
                return 1;
            }
        }

        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        Application.Run(new SetupForm(options));
        return 0;
    }
}

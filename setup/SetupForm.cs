using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace TetherSetup;

internal sealed class SetupForm : Form
{
    private readonly SetupOptions _options;
    private readonly float _scale;
    private readonly Font _small = Theme.Body(8.5f);
    private readonly Label _title = new();
    private readonly Label _subtitle = new();
    private readonly Panel _page = new();
    private Action _primaryAction;
    private bool _busy;

    public SetupForm(SetupOptions options)
    {
        _options = options;
        using (var g = Graphics.FromHwnd(IntPtr.Zero)) _scale = g.DpiX / 96f;
        using (var stream = Resource("tether.ico")) Icon = new Icon(stream);

        AutoScaleMode = AutoScaleMode.None;
        FormBorderStyle = FormBorderStyle.None;
        StartPosition = FormStartPosition.CenterScreen;
        ClientSize = new Size(S(700), S(420));
        BackColor = Theme.Background;
        ForeColor = Theme.Text;
        Font = Theme.Body(10f);
        Text = options.Mode == Mode.Uninstall ? "Uninstall Tether" : "Tether Setup";
        KeyPreview = true;
        DoubleBuffered = true;

        var close = new CloseButton { Bounds = new Rectangle(ClientSize.Width - S(50), S(10), S(40), S(32)) };
        close.Click += (_, _) => Close();
        _title.SetBounds(S(300), S(52), S(370), S(52));
        _title.Font = Theme.Display(24f);
        _title.ForeColor = Theme.Text;
        _subtitle.SetBounds(S(302), S(106), S(360), S(64));
        _subtitle.Font = Theme.Body(10.5f);
        _subtitle.ForeColor = Theme.Muted;
        _page.SetBounds(S(302), S(178), S(360), S(222));
        Controls.AddRange(new Control[] { close, _title, _subtitle, _page });

        switch (options.Mode)
        {
            case Mode.Uninstall: ShowUninstall(); break;
            case Mode.Update: Shown += (_, _) => StartInstall(); break; // the updater already asked
            default: ShowWelcome(); break;
        }
    }

    private int S(int value) => (int)Math.Round(value * _scale);

    private static Stream Resource(string name) => Assembly.GetExecutingAssembly().GetManifestResourceStream(name);

    // ---------- pages ----------
    private void ShowWelcome()
    {
        _page.Controls.Clear();
        _title.Text = "Tether";
        _subtitle.Text = "Your Android phone as a webcam. Tether Camera shows up in Discord, OBS, Zoom and every other app.";

        bool installed = Installer.IsInstalled(_options.Folder);
        var desktop = new CheckOption { Text = "Add a desktop shortcut", Checked = _options.DesktopShortcut, Bounds = new Rectangle(0, 0, S(360), S(28)) };
        var folder = SmallLabel($"Installs to {_options.Folder}", 0, S(46));

        string noteText = installed
            ? "Tether is already installed and gets updated. Your settings stay as they are."
            : "Windows asks for admin rights once, to add Tether Camera to this PC.";
        if (Installer.IsRunning()) noteText += " Tether is running and will be closed first.";
        bool canInstall = Installer.HasPayload && Installer.IsWindows11();
        if (!Installer.IsWindows11()) noteText = "Tether Camera needs Windows 11. This PC runs an older Windows, so Tether can't be installed here.";
        if (!Installer.HasPayload) noteText = "This setup file is incomplete. Download it again from the Tether page on GitHub.";
        var note = SmallLabel(noteText, 0, S(80));
        note.ForeColor = Theme.Muted;

        var install = new FlatButton { Primary = true, Text = installed ? "Update" : "Install", Bounds = new Rectangle(S(240), S(178), S(120), S(40)), Enabled = canInstall };
        var cancel = new FlatButton { Text = "Cancel", Bounds = new Rectangle(S(130), S(178), S(100), S(40)) };
        _primaryAction = () =>
        {
            if (!install.Enabled) return;
            _options.DesktopShortcut = desktop.Checked;
            StartInstall();
        };
        install.Click += (_, _) => _primaryAction();
        cancel.Click += (_, _) => Close();
        _page.Controls.AddRange(new Control[] { desktop, folder, note, install, cancel });
        install.Select();
    }

    private async void StartInstall()
    {
        _busy = true;
        _primaryAction = null;
        bool update = _options.Mode == Mode.Update;
        var (status, bar, percent) = ShowProgress(update ? "Updating" : "Installing", "This takes a few seconds.");
        try
        {
            await Task.Run(() => Installer.Install(_options, (value, text) => BeginInvoke((Action)(() =>
            {
                bar.Value = value;
                status.Text = text;
                percent.Text = $"{(int)(value * 100)}%";
            }))));
            _busy = false;
            if (update)
            {
                // Back to where the user was: start the new Tether and get out of the way.
                Installer.Launch(_options.Folder);
                Close();
                return;
            }
            ShowDone();
        }
        catch (Exception ex)
        {
            _busy = false;
            ShowError(ex.Message, "Setup couldn't finish. Your settings weren't changed.");
        }
    }

    private void ShowDone()
    {
        _page.Controls.Clear();
        _title.Text = "All set";
        _subtitle.Text = "Tether waits in the tray next to the clock. Plug in your phone, then pick \"Tether Camera\" in your app.";
        var hintText = SmallLabel("First time? Tether's Setup tab puts the app on your phone and shows how to turn on USB debugging.", 0, 0);
        hintText.ForeColor = Theme.Muted;
        var open = new FlatButton { Primary = true, Text = "Open Tether", Bounds = new Rectangle(S(220), S(178), S(140), S(40)) };
        var close = new FlatButton { Text = "Close", Bounds = new Rectangle(S(110), S(178), S(100), S(40)) };
        _primaryAction = () =>
        {
            Installer.Launch(_options.Folder);
            Close();
        };
        open.Click += (_, _) => _primaryAction();
        close.Click += (_, _) => Close();
        _page.Controls.AddRange(new Control[] { hintText, open, close });
        open.Select();
    }

    private void ShowUninstall()
    {
        _page.Controls.Clear();
        _title.Text = "Uninstall Tether";
        _subtitle.Text = "Removes Tether and Tether Camera from this PC. Your settings stay, in case you come back.";
        string noteText = Installer.IsRunning() ? "Tether is running and will be closed first." : "The app on your phone stays; uninstall it there if you like.";
        var note = SmallLabel(noteText, 0, 0);
        note.ForeColor = Theme.Muted;
        var remove = new FlatButton { Primary = true, Text = "Uninstall", Bounds = new Rectangle(S(240), S(178), S(120), S(40)) };
        var cancel = new FlatButton { Text = "Cancel", Bounds = new Rectangle(S(130), S(178), S(100), S(40)) };
        _primaryAction = StartUninstall;
        remove.Click += (_, _) => _primaryAction();
        cancel.Click += (_, _) => Close();
        _page.Controls.AddRange(new Control[] { note, remove, cancel });
        remove.Select();
    }

    private async void StartUninstall()
    {
        _busy = true;
        _primaryAction = null;
        var (status, bar, percent) = ShowProgress("Uninstalling", "This takes a few seconds.");
        try
        {
            await Task.Run(() => Installer.Uninstall((value, text) => BeginInvoke((Action)(() =>
            {
                bar.Value = value;
                status.Text = text;
                percent.Text = $"{(int)(value * 100)}%";
            }))));
            _busy = false;
            _page.Controls.Clear();
            _title.Text = "Tether is gone";
            _subtitle.Text = "Tether and Tether Camera were removed from this PC.";
            var close = new FlatButton { Primary = true, Text = "Close", Bounds = new Rectangle(S(240), S(178), S(120), S(40)) };
            close.Click += (_, _) => Close();
            _primaryAction = Close;
            _page.Controls.Add(close);
            close.Select();
        }
        catch (Exception ex)
        {
            _busy = false;
            ShowError(ex.Message, "Uninstalling couldn't finish.");
        }
    }

    private (Label status, ThinProgress bar, Label percent) ShowProgress(string title, string subtitle)
    {
        _page.Controls.Clear();
        _title.Text = title;
        _subtitle.Text = subtitle;
        var status = new Label { Text = "Getting ready", AutoSize = true, Location = new Point(0, S(20)), Font = Theme.Strong(10.5f), ForeColor = Theme.Text };
        var bar = new ThinProgress { Bounds = new Rectangle(0, S(56), S(360), S(6)) };
        var percent = SmallLabel("0%", 0, S(72));
        _page.Controls.AddRange(new Control[] { status, bar, percent });
        return (status, bar, percent);
    }

    private void ShowError(string message, string subtitle)
    {
        _page.Controls.Clear();
        _title.Text = "That didn't work";
        _subtitle.Text = subtitle;
        var detail = SmallLabel(message, 0, 0);
        detail.ForeColor = Theme.Muted;
        var close = new FlatButton { Primary = true, Text = "Close", Bounds = new Rectangle(S(240), S(178), S(120), S(40)) };
        close.Click += (_, _) => Close();
        _primaryAction = Close;
        _page.Controls.AddRange(new Control[] { detail, close });
    }

    private Label SmallLabel(string text, int x, int y) => new()
    {
        Text = text, AutoSize = true, MaximumSize = new Size(S(360), 0), Location = new Point(x, y), Font = _small, ForeColor = Theme.Faint,
    };

    // ---------- window ----------
    protected override CreateParams CreateParams
    {
        get
        {
            var cp = base.CreateParams;
            cp.ClassStyle |= 0x20000; // CS_DROPSHADOW
            return cp;
        }
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        int round = 2; // DWMWCP_ROUND, Windows 11 only
        DwmSetWindowAttribute(Handle, 33, ref round, sizeof(int));
    }

    /// <summary>The left side: the Tether mark (a lens on a cable), big and calm.</summary>
    protected override void OnPaint(PaintEventArgs e)
    {
        var g = e.Graphics;
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.PixelOffsetMode = PixelOffsetMode.HighQuality;

        var art = new Rectangle(0, 0, S(260), ClientSize.Height);
        using (var brush = new SolidBrush(Theme.Panel)) g.FillRectangle(brush, art);

        float u = S(170) / 512f;
        float ox = (art.Width - 512 * u) / 2f, oy = (art.Height - 512 * u) / 2f - S(14);
        using (var pen = new Pen(Theme.Text, 36 * u) { StartCap = LineCap.Round, EndCap = LineCap.Round })
        {
            g.DrawEllipse(pen, ox + 144 * u, oy + 104 * u, 224 * u, 224 * u);
            g.DrawLine(pen, ox + 256 * u, oy + 346 * u, ox + 256 * u, oy + 430 * u);
        }
        using (var dot = new SolidBrush(Theme.Text)) g.FillEllipse(dot, ox + 222 * u, oy + 182 * u, 68 * u, 68 * u);
        TextRenderer.DrawText(g, $"Version {Installer.Version}", _small, new Point(S(22), ClientSize.Height - S(34)), Theme.Faint);

        using var line = new Pen(Theme.Line);
        g.DrawLine(line, art.Right, 0, art.Right, ClientSize.Height);
        g.DrawRectangle(line, 0, 0, ClientSize.Width - 1, ClientSize.Height - 1);
    }

    protected override void OnMouseDown(MouseEventArgs e)
    {
        base.OnMouseDown(e);
        if (e.Button != MouseButtons.Left) return;
        ReleaseCapture();
        SendMessage(Handle, 0xA1 /* WM_NCLBUTTONDOWN */, (IntPtr)2 /* HTCAPTION */, IntPtr.Zero);
    }

    protected override void OnKeyDown(KeyEventArgs e)
    {
        base.OnKeyDown(e);
        if (e.KeyCode == Keys.Escape) Close();
        else if (e.KeyCode == Keys.Enter && _primaryAction != null && !(ActiveControl is CheckOption)) _primaryAction();
    }

    protected override void OnFormClosing(FormClosingEventArgs e)
    {
        if (_busy) e.Cancel = true; // half copied files would help nobody
        base.OnFormClosing(e);
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing) _small.Dispose();
        base.Dispose(disposing);
    }

    [DllImport("dwmapi.dll")] private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int value, int size);
    [DllImport("user32.dll")] private static extern bool ReleaseCapture();
    [DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam);
}

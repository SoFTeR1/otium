// PS5 Launcher web installer: a small branded WPF window that downloads the latest release from GitHub,
// runs the NSIS package silently and launches the app. Compiled with the .NET Framework csc (C# 5).
using System;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net;
using System.Reflection;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Markup;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using Microsoft.Win32;

[assembly: AssemblyTitle("PS5 Launcher Installer")]
[assembly: AssemblyProduct("PS5 Launcher")]
[assembly: AssemblyCompany("SoFTeR1")]
[assembly: AssemblyVersion("__VERSION__.0")]

public static class Program
{
    [STAThread]
    public static void Main()
    {
        var app = new Application();
        app.Run(new InstallerWindow());
    }
}

public class InstallerWindow : Window
{
    const string Releases = "https://github.com/SoFTeR1/ps5-launcher/releases/latest/download/";
    // electron-builder keeps the previous install folder here (derived from the appId)
    const string RegKey = @"Software\ab98e456-26bd-5f85-9fce-95c7943e0e2f";
    const long UnpackedBytes = __BYTES__;

    FrameworkElement root;
    string installDir;
    // --dry-run: real download, simulated install — for checking the UI without touching the PC
    readonly bool dryRun = Environment.GetCommandLineArgs().Contains("--dry-run");
    string setupFile;
    string version;
    bool busy;

    T Get<T>(string name) where T : class { return (T)root.FindName(name); }

    public InstallerWindow()
    {
        Title = "Установка PS5 Launcher";
        Width = 820; Height = 500;
        WindowStyle = WindowStyle.None;
        AllowsTransparency = true;
        Background = Brushes.Transparent;
        ResizeMode = ResizeMode.NoResize;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        Opacity = 0;

        using (var s = Assembly.GetExecutingAssembly().GetManifestResourceStream("ui.xaml"))
            root = (FrameworkElement)XamlReader.Load(s);
        Content = root;

        var icon = new BitmapImage();
        icon.BeginInit();
        icon.StreamSource = Assembly.GetExecutingAssembly().GetManifestResourceStream("icon.png");
        icon.CacheOption = BitmapCacheOption.OnLoad;
        icon.EndInit();
        Get<Image>("Logo").Source = icon;
        Icon = icon;

        root.MouseLeftButtonDown += (s, e) => { if (e.ButtonState == MouseButtonState.Pressed) DragMove(); };
        Get<Button>("BtnClose").Click += (s, e) => { if (!busy) Close(); };
        Get<Button>("BtnMin").Click += (s, e) => WindowState = WindowState.Minimized;
        Get<Button>("BtnInstall").Click += (s, e) => Install();
        Get<Button>("BtnRetry").Click += (s, e) => Install();
        Get<Button>("BtnChange").Click += (s, e) => ChooseFolder();
        Get<Button>("BtnLaunch").Click += (s, e) => { Launch(); Close(); };
        Get<Button>("BtnDone").Click += (s, e) => Close();

        installDir = DefaultDir();
        Get<TextBlock>("PathText").Text = installDir;

        Loaded += async (s, e) =>
        {
            BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(450)));
            await LoadVersion();
        };
    }

    static string DefaultDir()
    {
        // a running launcher knows best where it lives
        try
        {
            foreach (var p in Process.GetProcessesByName("PS5 Launcher"))
            {
                var dir = Path.GetDirectoryName(p.MainModule.FileName);
                if (Directory.Exists(dir)) return dir;
            }
        }
        catch { }
        try
        {
            var lnk = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs), "PS5 Launcher.lnk");
            if (File.Exists(lnk))
            {
                dynamic shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell"));
                string target = shell.CreateShortcut(lnk).TargetPath;
                if (File.Exists(target)) return Path.GetDirectoryName(target);
            }
        }
        catch { }
        try
        {
            using (var k = Registry.CurrentUser.OpenSubKey(RegKey))
            {
                var v = k == null ? null : k.GetValue("InstallLocation") as string;
                if (!string.IsNullOrEmpty(v) && Directory.Exists(v)) return v;
            }
        }
        catch { }
        return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "PS5 Launcher");
    }

    static WebClient Client()
    {
        ServicePointManager.SecurityProtocol = SecurityProtocolType.Tls12;
        var wc = new WebClient();
        wc.Headers.Add("User-Agent", "PS5LauncherInstaller");
        return wc;
    }

    async Task LoadVersion()
    {
        try
        {
            string yml;
            using (var wc = Client()) yml = await wc.DownloadStringTaskAsync(Releases + "latest.yml");
            foreach (var line in yml.Split('\n'))
            {
                var l = line.Trim();
                if (l.StartsWith("version:")) version = l.Substring(8).Trim().Trim('\'', '"');
                if (l.StartsWith("path:")) setupFile = l.Substring(5).Trim().Trim('\'', '"');
            }
            Get<TextBlock>("VersionText").Text = "Версия " + version;
            bool upgrade = File.Exists(Path.Combine(installDir, "PS5 Launcher.exe"));
            Get<TextBlock>("WelcomeTitle").Text = upgrade ? "Обновление готово к установке" : "Всё готово к установке";
            Get<TextBlock>("BtnInstallText").Text = upgrade ? "Обновить" : "Установить";
        }
        catch
        {
            Get<TextBlock>("VersionText").Text = "Нет связи с GitHub";
        }
    }

    void ChooseFolder()
    {
        using (var d = new System.Windows.Forms.FolderBrowserDialog())
        {
            d.Description = "Куда установить PS5 Launcher?";
            d.SelectedPath = Directory.Exists(installDir) ? installDir : Path.GetDirectoryName(installDir);
            if (d.ShowDialog() != System.Windows.Forms.DialogResult.OK) return;
            var p = d.SelectedPath;
            if (!p.TrimEnd('\\').EndsWith("PS5 Launcher", StringComparison.OrdinalIgnoreCase)) p = Path.Combine(p, "PS5 Launcher");
            installDir = p;
            Get<TextBlock>("PathText").Text = installDir;
        }
    }

    void Show(string panel)
    {
        foreach (var name in new[] { "PWelcome", "PProgress", "PDone", "PError" })
        {
            var el = Get<FrameworkElement>(name);
            if (name != panel) { el.Visibility = Visibility.Collapsed; continue; }
            el.Visibility = Visibility.Visible;
            var tt = new TranslateTransform(24, 0);
            el.RenderTransform = tt;
            el.BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(380)));
            tt.BeginAnimation(TranslateTransform.XProperty, new DoubleAnimation(24, 0, TimeSpan.FromMilliseconds(420)) { EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut } });
        }
    }

    void SetProgress(double p, string status, string detail)
    {
        p = Math.Max(0, Math.Min(1, p));
        Get<TextBlock>("Percent").Text = Math.Round(p * 100) + "%";
        Get<TextBlock>("Status").Text = status;
        Get<TextBlock>("Detail").Text = detail;
        var track = Get<FrameworkElement>("Track");
        var fill = Get<FrameworkElement>("Fill");
        fill.BeginAnimation(WidthProperty, new DoubleAnimation(Math.Max(8, track.ActualWidth * p), TimeSpan.FromMilliseconds(250)));
    }

    async void Install()
    {
        if (busy) return;
        busy = true;
        Show("PProgress");
        try
        {
            if (setupFile == null) await LoadVersion();
            if (setupFile == null) throw new Exception("Не удалось получить информацию о последней версии с GitHub. Проверьте подключение к интернету.");

            // 1. download (0–60%)
            var tmp = Path.Combine(Path.GetTempPath(), setupFile);
            using (var wc = Client())
            {
                wc.DownloadProgressChanged += (s, e) =>
                {
                    double total = e.TotalBytesToReceive > 0 ? e.TotalBytesToReceive : 82e6;
                    SetProgress(0.6 * e.BytesReceived / total, "Загрузка PS5 Launcher " + version,
                        string.Format("{0:0.0} из {1:0.0} МБ", e.BytesReceived / 1048576.0, total / 1048576.0));
                };
                await wc.DownloadFileTaskAsync(new Uri(Releases + setupFile), tmp);
            }

            if (dryRun)
            {
                for (int i = 0; i <= 20; i++)
                {
                    SetProgress(0.6 + 0.4 * i / 20, "Установка…", "Распаковка файлов в " + installDir);
                    await Task.Delay(150);
                }
                try { File.Delete(tmp); } catch { }
                Show("PDone");
                return;
            }

            // 2. close a running launcher so its files can be replaced
            SetProgress(0.6, "Подготовка…", "");
            foreach (var p in Process.GetProcessesByName("PS5 Launcher"))
            {
                try { p.CloseMainWindow(); if (!p.WaitForExit(2500)) p.Kill(); } catch { }
            }

            // 3. silent install (60–100%); NSIS reads the folder from the registry, /D covers fresh installs
            using (var k = Registry.CurrentUser.CreateSubKey(RegKey)) k.SetValue("InstallLocation", installDir);
            var started = DateTime.Now;
            var proc = Process.Start(new ProcessStartInfo(tmp, "/S /D=" + installDir) { UseShellExecute = false });
            while (!proc.HasExited)
            {
                await Task.Delay(200);
                double bySize = UnpackedBytes > 0 ? DirSize(installDir) / (double)UnpackedBytes : 0;
                double byTime = 1 - Math.Exp(-(DateTime.Now - started).TotalSeconds / 9);
                SetProgress(0.6 + 0.38 * Math.Min(1, Math.Max(bySize, byTime)), "Установка…", "Распаковка файлов в " + installDir);
            }
            if (proc.ExitCode != 0) throw new Exception("Установщик завершился с кодом " + proc.ExitCode + ".");
            try { File.Delete(tmp); } catch { }

            SetProgress(1, "Готово", "");
            await Task.Delay(400);
            Show("PDone");
        }
        catch (Exception ex)
        {
            Get<TextBlock>("ErrorText").Text = ex.Message;
            Show("PError");
        }
        finally
        {
            busy = false;
        }
    }

    static long DirSize(string dir)
    {
        try
        {
            if (!Directory.Exists(dir)) return 0;
            return new DirectoryInfo(dir).EnumerateFiles("*", SearchOption.AllDirectories).Sum(f => f.Length);
        }
        catch { return 0; }
    }

    void Launch()
    {
        try { Process.Start(Path.Combine(installDir, "PS5 Launcher.exe")); } catch { }
    }
}

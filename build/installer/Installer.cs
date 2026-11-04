// OTIUM web installer: a small branded WPF window that downloads the latest release from GitHub,
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

[assembly: AssemblyTitle("OTIUM Installer")]
[assembly: AssemblyProduct("OTIUM")]
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
    const string Releases = "https://github.com/SoFTeR1/otium/releases/latest/download/";
    // the app was called "PS5 Launcher" before 2.0; installs from then are upgraded in place
    static readonly string[] ExeNames = { "OTIUM", "PS5 Launcher" };
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
        Title = "Установка OTIUM";
        Width = 860; Height = 520;
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
        Get<Image>("IntroLogo").Source = icon;
        Icon = icon;
        AddStars();

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
            var intro = PlayIntro();
            await LoadVersion();
            await intro;
        };
    }

    // ---------- look ----------
    static readonly Random Rnd = new Random();
    static Duration Ms(double ms) { return new Duration(TimeSpan.FromMilliseconds(ms)); }
    static DoubleAnimation Anim(double from, double to, double ms, double delay = 0, IEasingFunction ease = null)
    {
        return new DoubleAnimation(from, to, Ms(ms))
        {
            BeginTime = TimeSpan.FromMilliseconds(delay),
            EasingFunction = ease ?? new CubicEase { EasingMode = EasingMode.EaseOut },
            FillBehavior = FillBehavior.HoldEnd
        };
    }

    // soft lights scattered over the window, each breathing at its own pace
    void AddStars()
    {
        var canvas = Get<Canvas>("Stars");
        for (int i = 0; i < 46; i++)
        {
            double size = 1.5 + Math.Pow(Rnd.NextDouble(), 3) * 7;
            var dot = new System.Windows.Shapes.Ellipse { Width = size * 3, Height = size * 3, Opacity = 0 };
            var b = new RadialGradientBrush();
            b.GradientStops.Add(new GradientStop(Color.FromArgb(255, 235, 244, 255), 0));
            b.GradientStops.Add(new GradientStop(Color.FromArgb(120, 150, 190, 255), 0.25));
            b.GradientStops.Add(new GradientStop(Color.FromArgb(0, 120, 170, 255), 1));
            dot.Fill = b;
            Canvas.SetLeft(dot, Rnd.NextDouble() * 830);
            Canvas.SetTop(dot, Rnd.NextDouble() * 490);
            canvas.Children.Add(dot);
            var tw = new DoubleAnimation(0.08 + Rnd.NextDouble() * 0.2, 0.45 + Rnd.NextDouble() * 0.5, Ms(1600 + Rnd.NextDouble() * 2600))
            {
                AutoReverse = true,
                RepeatBehavior = RepeatBehavior.Forever,
                BeginTime = TimeSpan.FromMilliseconds(Rnd.NextDouble() * 2500),
                EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut }
            };
            dot.BeginAnimation(OpacityProperty, tw);
        }
        // the aura breathes; a band of light crosses the window every few seconds
        Get<FrameworkElement>("Aura").BeginAnimation(OpacityProperty, new DoubleAnimation(0.38, 0.62, Ms(4200))
        {
            AutoReverse = true,
            RepeatBehavior = RepeatBehavior.Forever,
            EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut }
        });
        var sweep = new DoubleAnimationUsingKeyFrames { RepeatBehavior = RepeatBehavior.Forever, BeginTime = TimeSpan.FromMilliseconds(1600) };
        sweep.KeyFrames.Add(new DiscreteDoubleKeyFrame(-400, KeyTime.FromTimeSpan(TimeSpan.Zero)));
        sweep.KeyFrames.Add(new EasingDoubleKeyFrame(1200, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(2600)), new SineEase { EasingMode = EasingMode.EaseInOut }));
        sweep.KeyFrames.Add(new DiscreteDoubleKeyFrame(1200, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(9000))));
        ((TranslateTransform)root.FindName("SweepX")).BeginAnimation(TranslateTransform.XProperty, sweep);
    }

    // the mark alone in the middle, then it hands over to the installer
    async Task PlayIntro()
    {
        var logo = Get<FrameworkElement>("IntroLogo");
        var word = Get<FrameworkElement>("IntroWord");
        var scale = (ScaleTransform)root.FindName("IntroScale");
        var back = new BackEase { Amplitude = 0.35, EasingMode = EasingMode.EaseOut };
        logo.BeginAnimation(OpacityProperty, Anim(0, 1, 900, 150));
        scale.BeginAnimation(ScaleTransform.ScaleXProperty, Anim(0.82, 1, 1100, 150, back));
        scale.BeginAnimation(ScaleTransform.ScaleYProperty, Anim(0.82, 1, 1100, 150, back));
        var wt = new TranslateTransform(0, 14);
        word.RenderTransform = wt;
        word.BeginAnimation(OpacityProperty, Anim(0, 1, 900, 650));
        wt.BeginAnimation(TranslateTransform.YProperty, Anim(14, 0, 1000, 650));
        await Task.Delay(2300);

        var intro = Get<FrameworkElement>("Intro");
        var st = new ScaleTransform(1, 1, 414, 244);
        intro.RenderTransform = st;
        intro.BeginAnimation(OpacityProperty, Anim(1, 0, 650));
        st.BeginAnimation(ScaleTransform.ScaleXProperty, Anim(1, 1.08, 800));
        st.BeginAnimation(ScaleTransform.ScaleYProperty, Anim(1, 1.08, 800));
        var main = Get<FrameworkElement>("Main");
        var mt = new TranslateTransform(0, 18);
        main.RenderTransform = mt;
        main.BeginAnimation(OpacityProperty, Anim(0, 1, 700, 250));
        mt.BeginAnimation(TranslateTransform.YProperty, Anim(18, 0, 800, 250));
        await Task.Delay(1000);
        intro.Visibility = Visibility.Collapsed;
    }

    static string DefaultDir()
    {
        // a running launcher knows best where it lives
        try
        {
            foreach (var name in ExeNames)
                foreach (var p in Process.GetProcessesByName(name))
                {
                    var dir = Path.GetDirectoryName(p.MainModule.FileName);
                    if (Directory.Exists(dir)) return dir;
                }
        }
        catch { }
        foreach (var name in ExeNames)
        {
            try
            {
                var lnk = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Programs), name + ".lnk");
                if (File.Exists(lnk))
                {
                    dynamic shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell"));
                    string target = shell.CreateShortcut(lnk).TargetPath;
                    if (File.Exists(target)) return Path.GetDirectoryName(target);
                }
            }
            catch { }
        }
        try
        {
            using (var k = Registry.CurrentUser.OpenSubKey(RegKey))
            {
                var v = k == null ? null : k.GetValue("InstallLocation") as string;
                if (!string.IsNullOrEmpty(v) && Directory.Exists(v)) return v;
            }
        }
        catch { }
        return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "OTIUM");
    }

    static WebClient Client()
    {
        ServicePointManager.SecurityProtocol = SecurityProtocolType.Tls12;
        var wc = new WebClient();
        wc.Headers.Add("User-Agent", "OTIUMInstaller");
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
            bool upgrade = ExeNames.Any(n => File.Exists(Path.Combine(installDir, n + ".exe")));
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
            d.Description = "Куда установить OTIUM?";
            d.SelectedPath = Directory.Exists(installDir) ? installDir : Path.GetDirectoryName(installDir);
            if (d.ShowDialog() != System.Windows.Forms.DialogResult.OK) return;
            var p = d.SelectedPath;
            var leaf = Path.GetFileName(p.TrimEnd('\\'));
            if (!ExeNames.Any(n => leaf.Equals(n, StringComparison.OrdinalIgnoreCase))) p = Path.Combine(p, "OTIUM");
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
                    SetProgress(0.6 * e.BytesReceived / total, "Загрузка OTIUM " + version,
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
            foreach (var name in ExeNames)
                foreach (var p in Process.GetProcessesByName(name))
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
        try { Process.Start(Path.Combine(installDir, "OTIUM.exe")); } catch { }
    }
}

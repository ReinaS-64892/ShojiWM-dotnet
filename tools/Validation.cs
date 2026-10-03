using System.Diagnostics;

namespace ShojiWM.Tools;

internal static class Validation
{
    internal static void Run(string root)
    {
        UpstreamClean(root);
        try
        {
            BindingGenerator.Write(root);
            NativeAbiGenerator.Run(root, false);
            NativeOperationGenerator.Run(root, false);
            RunProcess(root, "rustfmt", ["--check", "--edition", "2024", "src/lib.rs", "src/main.rs", "tests/runtime.rs"]);
            BindingGenerator.Check(root);
            NativeAbiGenerator.Run(root, true);
            NativeOperationGenerator.Run(root, true);
            GeneratorTests.Run(root);
            RunProcess(root, "cargo", ["build", "--locked", "--offline"]);
            RunProcess(root, "cargo", ["test", "--locked", "--offline"]);
            RunProcess(root, "dotnet", ["build", "dotnet/ShojiWM.Tests/ShojiWM.Tests.csproj", "-c", "Release", "--disable-build-servers", "-m:1"]);
            string runtime = Path.Combine(root, "dotnet/ShojiWM.Runtime/bin/Release/net10.0/ShojiWM.Runtime.dll");
            string fixture = Path.Combine(root, "dotnet/ShojiWM.Tests/bin/Release/net10.0/ShojiWM.Tests.dll");
            string example = Path.Combine(root, "dotnet/ShojiWM.Example/bin/Release/net10.0/ShojiWM.Example.dll");
            RunProcess(root, "dotnet", [fixture]);
            var environment = new Dictionary<string, string>
            {
                ["SHOJI_TEST_DOTNET_RUNTIME"] = runtime,
                ["SHOJI_TEST_DOTNET_CONFIG"] = example,
                ["SHOJI_TEST_DOTNET_FIXTURE"] = fixture,
            };
            RunProcess(root, "cargo", ["test", "--locked", "--offline", "--", "--ignored", "--test-threads=1"], environment);
            RunProcess(root, "cargo", ["run", "--locked", "--offline", "--manifest-path", "dotnet/NativeHost.Tests/Cargo.toml", "--target-dir", "target", "--", runtime, fixture]);
            string executable = Path.Combine(root, "target/debug/ShojiWM-dotnet" + (OperatingSystem.IsWindows() ? ".exe" : ""));
            RunProcess(root, executable, ["--help"]);
            RunProcess(root, executable, ["--version"]);
        }
        finally
        {
            UpstreamClean(root);
        }
    }

    internal static void UpstreamClean(string root)
    {
        string status = RunProcess(root, "git", ["-C", "ShojiWM", "status", "--porcelain"], capture: true);
        if (status.Length != 0) throw new InvalidOperationException("ShojiWM must remain unchanged:\n" + status);
    }

    // ArgumentList avoids shell quoting/injection and preserves paths containing spaces.
    // Inherit stdout/stderr for builds/tests; capture only the small git status response.
    internal static string RunProcess(string root, string executable, string[] arguments,
        IReadOnlyDictionary<string, string>? environment = null, bool capture = false)
    {
        var start = new ProcessStartInfo(executable)
        {
            WorkingDirectory = root,
            UseShellExecute = false,
            RedirectStandardOutput = capture,
        };
        foreach (string argument in arguments) start.ArgumentList.Add(argument);
        if (environment is not null)
            foreach (var (key, value) in environment) start.Environment[key] = value;
        if (!capture) Console.WriteLine($"> {executable} {string.Join(' ', arguments)}");
        using var process = Process.Start(start) ?? throw new InvalidOperationException($"Cannot start {executable}");
        string output = capture ? process.StandardOutput.ReadToEnd() : "";
        process.WaitForExit();
        if (process.ExitCode != 0)
            throw new InvalidOperationException($"{executable} exited with code {process.ExitCode}");
        return output;
    }
}

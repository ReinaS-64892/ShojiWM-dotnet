using System.Text;
using System.CommandLine;

namespace ShojiWM.Tools;

internal static class GeneratorTests
{
    internal static void Run(string root)
    {
        int passed = 0;
        Test("CLI parses subcommands and check option", () =>
        {
            var command = Program.CreateCommand();
            foreach (string name in new[] { "generate", "test", "validate" })
            {
                var result = command.Parse([name]);
                if (result.Errors.Count != 0) throw new Exception("valid command rejected");
                Equal(name, result.CommandResult.Command.Name);
            }
            var check = (Option<bool>)command.Subcommands.Single(item => item.Name == "generate").Options.Single(item => item.Name == "--check");
            if (command.Parse(["generate"]).GetValue(check) || !command.Parse(["generate", "--check"]).GetValue(check))
                throw new Exception("incorrect --check value");
        });
        Test("CLI root and subcommand help succeed", () =>
        {
            foreach (string[] args in new string[][] { [], ["--help"], ["generate", "--help"], ["test", "--help"], ["validate", "--help"] })
            {
                using var output = new StringWriter();
                using var error = new StringWriter();
                int code = Program.CreateCommand().Parse(args).Invoke(new InvocationConfiguration { Output = output, Error = error });
                if (code != 0 || error.ToString().Length != 0) throw new Exception("help failed");
                Contains(output.ToString(), "Usage:");
                if (args.FirstOrDefault() == "generate") Contains(output.ToString(), "--check");
            }
        });
        Test("CLI rejects invalid arguments before invoking actions", () =>
        {
            foreach (string[] args in new string[][] { ["unknown"], ["generate", "--unknown"], ["test", "--check"],
                ["validate", "extra"], ["generate", "--check", "invalid"] })
            {
                using var output = new StringWriter();
                using var error = new StringWriter();
                var result = Program.CreateCommand().Parse(args);
                if (result.Errors.Count == 0 || result.Invoke(new InvocationConfiguration { Output = output, Error = error }) == 0)
                    throw new Exception("invalid arguments accepted");
                if (error.ToString().Length == 0) throw new Exception("missing parse error diagnostic");
            }
        });
        Test("deterministic output", () => Equal(BindingGenerator.Generate(root), BindingGenerator.Generate(root)));
        Test("production numeric/optional/wire mappings", () =>
        {
            string output = BindingGenerator.Generate(root);
            foreach (string expected in new[] { "uint? Vendor", "string? AppId", "List<byte>? Bytes",
                "JsonStringEnumMemberName(\"xdg-decoration-v1\")", "JsonPropertyName(\"switch\")" })
                Contains(output, expected);
        });
        using (var fixture = new Fixture(root))
        {
            string path = Path.Combine(fixture.Root, "ShojiWM/src/shojiwm_lib/src/ssd/window_model.rs");
            string original = File.ReadAllText(path);
            Test("new upstream fields are included", () =>
            {
                File.WriteAllText(path, original.Replace("pub struct WaylandWindowSnapshot {",
                    "pub struct WaylandWindowSnapshot {\n    pub new_field: Option<bool>,"));
                Contains(BindingGenerator.Generate(fixture.Root), "public bool? NewField");
            });
            Test("unknown upstream types fail", () =>
            {
                File.WriteAllText(path, original.Replace("pub struct WaylandWindowSnapshot {",
                    "pub struct WaylandWindowSnapshot {\n    pub unsupported: HashSet<String>,"));
                Fails(() => BindingGenerator.Generate(fixture.Root), "unsupported Rust type");
            });
            Test("fail closed: data enum", () =>
            {
                File.WriteAllText(path, original.Replace("    XdgDecorationV1,", "    XdgDecorationV1(String),"));
                Fails(() => BindingGenerator.Generate(fixture.Root), "unsupported data enum");
            });
        }
        using (var fixture = new Fixture(root))
        {
            string path = Path.Combine(fixture.Root, BindingGenerator.Sources[0]);
            string original = File.ReadAllText(path);
            string expected = BindingGenerator.Generate(fixture.Root);
            Test("CRLF sources produce identical LF output", () =>
            {
                File.WriteAllText(path, original.Replace("\n", "\r\n"));
                Equal(expected, BindingGenerator.Generate(fixture.Root));
                File.WriteAllText(path, original);
            });
            Test("nested types and quoted commas", () =>
                Equal("BTreeMap<String, Vec<Option<u8>>>|#[serde(rename = \"a,b\")] pub field: String",
                    string.Join('|', BindingGenerator.SplitTop("BTreeMap<String, Vec<Option<u8>>>, #[serde(rename = \"a,b\")] pub field: String,"))));
            Test("ordinal output independent of culture", () =>
            {
                var saved = System.Globalization.CultureInfo.CurrentCulture;
                try
                {
                    System.Globalization.CultureInfo.CurrentCulture = System.Globalization.CultureInfo.GetCultureInfo("tr-TR");
                    Equal(expected, BindingGenerator.Generate(fixture.Root));
                }
                finally { System.Globalization.CultureInfo.CurrentCulture = saved; }
            });
            foreach (var (name, oldText, newText, error) in new (string, string, string, string)[]
            {
                ("unknown Rust type", "pub kind: String", "pub kind: HashSet<String>", "unsupported Rust type"),
                ("unknown serde attribute", "rename_all = \"camelCase\"", "deny_unknown_fields", "unsupported serde attribute"),
                ("unknown rename mode", "rename_all = \"camelCase\"", "rename_all = \"SCREAMING_SNAKE_CASE\"", "unsupported rename_all"),
                ("non-public field", "pub node_id: Option<String>", "node_id: Option<String>", "unsupported field"),
                ("unclosed model", "pub id: String,\n}", "pub id: String,", "unclosed model"),
            })
            {
                Test($"fail closed: {name}", () =>
                {
                    File.WriteAllText(path, original.Replace(oldText, newText));
                    Fails(() => BindingGenerator.Generate(fixture.Root), error);
                    File.WriteAllText(path, original);
                });
            }
            Test("check rejects missing output without creating it", () =>
            {
                Fails(() => BindingGenerator.Check(fixture.Root), "Generated bindings are stale");
                if (File.Exists(Path.Combine(fixture.Root, BindingGenerator.Output))) throw new Exception("check wrote output");
            });
            Test("write is UTF-8 without BOM, with LF, and check is read-only", () =>
            {
                BindingGenerator.Write(fixture.Root);
                byte[] actual = File.ReadAllBytes(Path.Combine(fixture.Root, BindingGenerator.Output));
                if (!actual.AsSpan().SequenceEqual(new UTF8Encoding(false).GetBytes(expected))) throw new Exception("output bytes differ");
                BindingGenerator.Check(fixture.Root);
            });
            Test("unchanged generation preserves timestamp", () =>
            {
                string output = Path.Combine(fixture.Root, BindingGenerator.Output);
                File.SetLastWriteTimeUtc(output, new DateTime(2020, 1, 1, 0, 0, 0, DateTimeKind.Utc));
                var before = File.GetLastWriteTimeUtc(output);
                BindingGenerator.Write(fixture.Root);
                if (File.GetLastWriteTimeUtc(output) != before) throw new Exception("unchanged output was rewritten");
            });
            Test("stale check fails without modifying output", () =>
            {
                string output = Path.Combine(fixture.Root, BindingGenerator.Output);
                File.WriteAllText(output, "stale");
                Fails(() => BindingGenerator.Check(fixture.Root), "Generated bindings are stale");
                Equal("stale", File.ReadAllText(output));
                BindingGenerator.Write(fixture.Root);
                BindingGenerator.Check(fixture.Root);
            });
            Test("invalid schema never overwrites existing output", () =>
            {
                File.WriteAllText(path, original.Replace("pub kind: String", "pub kind: HashSet<String>"));
                Fails(() => BindingGenerator.Write(fixture.Root), "unsupported Rust type");
                Equal(expected, File.ReadAllText(Path.Combine(fixture.Root, BindingGenerator.Output)));
                File.WriteAllText(path, original);
            });
        }
        using (var fixture = new Fixture(root))
        {
            string manifest = Path.Combine(fixture.Root, "tools/NativeAbi.schema.json");
            string original = File.ReadAllText(manifest);
            Test("native ABI generation and read-only check", () => {
                Fails(() => NativeAbiGenerator.Run(fixture.Root, true), "stale");
                NativeAbiGenerator.Run(fixture.Root, false);
                NativeAbiGenerator.Run(fixture.Root, true);
                string output = Path.Combine(fixture.Root, "src/bridge/native_generated.rs");
                File.SetLastWriteTimeUtc(output, new DateTime(2020, 1, 1));
                NativeAbiGenerator.Run(fixture.Root, false);
                if (File.GetLastWriteTimeUtc(output).Year != 2020) throw new Exception("native output rewritten");
                Contains(File.ReadAllText(output), "manifest_layouts");
            });
            Test("native ABI unknown types fail before writes", () => {
                string output = Path.Combine(fixture.Root, "src/bridge/native_generated.rs"), before = File.ReadAllText(output);
                File.WriteAllText(manifest, original.Replace("List<byte>", "List<UnknownAbiType>"));
                Fails(() => NativeAbiGenerator.Run(fixture.Root, false), "unsupported ABI type");
                Equal(before, File.ReadAllText(output));
                File.WriteAllText(manifest, original);
            });
        }
        using (var fixture = new Fixture(root))
        {
            string output = Path.Combine(fixture.Root, NativeOperationGenerator.EntryOutput);
            Test("operation generation check is read-only and detects missing output", () => {
                Fails(() => NativeOperationGenerator.Run(fixture.Root, true), "stale");
                if (File.Exists(output)) throw new Exception("operation check wrote output");
                NativeOperationGenerator.Run(fixture.Root, false);
                NativeOperationGenerator.Run(fixture.Root, true);
            });
            Test("operation generation is deterministic and preserves unchanged timestamp", () => {
                File.SetLastWriteTimeUtc(output, new DateTime(2020, 1, 1));
                string before = File.ReadAllText(output);
                NativeOperationGenerator.Run(fixture.Root, false);
                Equal(before, File.ReadAllText(output));
                if (File.GetLastWriteTimeUtc(output).Year != 2020) throw new Exception("operation output rewritten");
            });
            Test("stale operation check preserves output until regenerated", () => {
                File.WriteAllText(output, "stale");
                Fails(() => NativeOperationGenerator.Run(fixture.Root, true), "stale");
                Equal("stale", File.ReadAllText(output));
                NativeOperationGenerator.Run(fixture.Root, false);
                NativeOperationGenerator.Run(fixture.Root, true);
            });
        }
        Test("subprocess propagates nonzero exit", () =>
            Fails(() => Validation.RunProcess(root, "git", ["--not-a-real-option"], capture: true), "exited with code"));
        Console.WriteLine($"Generator tests: {passed} passed");

        void Test(string name, Action action)
        {
            try { action(); }
            catch (Exception exception) { throw new InvalidOperationException($"FAIL {name}: {exception.Message}", exception); }
            passed++;
            Console.WriteLine($"PASS {name}");
        }
    }

    private static void Equal(string expected, string actual)
    {
        if (!string.Equals(expected, actual, StringComparison.Ordinal)) throw new Exception("values differ");
    }
    private static void Contains(string actual, string expected)
    {
        if (!actual.Contains(expected, StringComparison.Ordinal)) throw new Exception($"missing: {expected}");
    }
    private static void Fails(Action action, string message)
    {
        try { action(); }
        catch (InvalidOperationException exception) when (exception.Message.Contains(message, StringComparison.Ordinal)) { return; }
        throw new Exception($"expected failure: {message}");
    }

    private sealed class Fixture : IDisposable
    {
        internal string Root { get; } = Path.Combine(Path.GetTempPath(), "shoji-generator tests " + Guid.NewGuid().ToString("N"));
        internal Fixture(string root)
        {
            foreach (string source in BindingGenerator.Sources.Append("tools/NativeAbi.schema.json"))
            {
                string target = Path.Combine(Root, source);
                Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                File.Copy(Path.Combine(root, source), target);
            }
        }
        public void Dispose() => Directory.Delete(Root, recursive: true);
    }
}

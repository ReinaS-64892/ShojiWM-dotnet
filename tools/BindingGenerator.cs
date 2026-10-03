using System.Text;
using System.Text.RegularExpressions;

namespace ShojiWM.Tools;

// Intentionally restricted parser, matching the original generator's DTO closure.
// Tagged/data unions use reviewed converters; unsupported Rust/serde syntax fails closed.
internal static class BindingGenerator
{
    internal static readonly string[] Sources =
    [
        "ShojiWM/src/shojiwm_lib/src/ssd/bridge.rs",
        "ShojiWM/src/shojiwm_lib/src/ssd/window_model.rs",
        "ShojiWM/src/shojiwm_lib/src/ssd/interaction.rs",
        "ShojiWM/src/shojiwm_lib/src/ssd/evaluator.rs",
        "ShojiWM/src/shojiwm_lib/src/runtime_input.rs",
        "ShojiWM/src/shojiwm_lib/src/runtime_debug.rs",
    ];

    internal const string Output = "dotnet/ShojiWM/Generated/Protocol.g.cs";
    private static readonly UTF8Encoding Utf8 = new(false, true);
    private static readonly string[] RootTypes =
        ["WaylandWindowSnapshot", "WaylandOutputSnapshot", "RuntimeInputDeviceSnapshot", "WireDecorationNode", "RuntimeWindowAction", "RuntimeDebugConfigUpdate", "WireRuntimeHandler", "WireWindowAction"];
    private static readonly Dictionary<string, string> Custom = new(StringComparer.Ordinal)
    {
        ["WireDecorationChild"] = "WireDecorationNode", // MVP: node children only
        ["WireDimension"] = "Dimension",
        ["WireFontFamily"] = "FontFamily",
        ["WireOnClick"] = "ClickDescriptor",
        ["WireResizeHitArea"] = "ResizeHitArea",
        ["WireCompiledEffect"] = "JsonElement",
        ["ManagedWindowAnimationSnapshot"] = "JsonElement",
    };
    private static readonly Dictionary<string, string> Scalars = new(StringComparer.Ordinal)
    {
        ["String"] = "string",
        ["str"] = "string",
        ["bool"] = "bool",
        ["i32"] = "int",
        ["u32"] = "uint",
        ["u64"] = "ulong",
        ["u8"] = "byte",
        ["f32"] = "float",
        ["f64"] = "double",
        ["serde_json::Value"] = "JsonElement",
    };
    private sealed record Model(string Kind, string Attributes, string Body, string Source);

    internal static List<string> SplitTop(string text, char separator = ',')
    {
        var parts = new List<string>();
        int start = 0, depth = 0;
        bool quoted = false;
        for (int i = 0; i < text.Length; i++)
        {
            char character = text[i];
            if (character == '"') quoted = !quoted;
            if (quoted) continue;
            if (character is '<' or '(' or '[' or '{') depth++;
            else if (">)]}".Contains(character)) depth--;
            else if (character == separator && depth == 0)
            {
                Add(text[start..i]);
                start = i + 1;
            }
        }
        Add(text[start..]);
        return parts;

        void Add(string value)
        {
            value = value.Trim();
            if (value.Length > 0) parts.Add(value);
        }
    }

    private static Dictionary<string, string?> Attributes(string text)
    {
        var attributes = new Dictionary<string, string?>(StringComparer.Ordinal);
        foreach (Match match in Regex.Matches(text, @"#\[serde\((.*?)\)\]", RegexOptions.Singleline))
            foreach (string item in SplitTop(match.Groups[1].Value))
            {
                int equals = item.IndexOf('=');
                string key = (equals < 0 ? item : item[..equals]).Trim();
                if (key is not ("rename" or "rename_all" or "default" or "skip_serializing_if"))
                    throw new InvalidOperationException($"unsupported serde attribute: {item}");
                attributes[key] = equals < 0 ? null : item[(equals + 1)..].Trim().Trim('"');
            }
        return attributes;
    }

    private static string Rename(string name, string? mode)
    {
        string[] words = Regex.Replace(name, @"([a-z0-9])([A-Z])", "$1_$2").Split('_');
        return mode switch
        {
            "camelCase" => words[0].ToLowerInvariant() + string.Concat(words.Skip(1).Select(Capitalize)),
            "kebab-case" => string.Join('-', words.Select(word => word.ToLowerInvariant())),
            "lowercase" => name.ToLowerInvariant(),
            null => name,
            _ => throw new InvalidOperationException($"unsupported rename_all: {mode}"),
        };
    }

    private static string Capitalize(string word) => word.Length == 0 ? word : char.ToUpperInvariant(word[0]) + word[1..].ToLowerInvariant();
    private static string Pascal(string name) => string.Concat(name.Split('_').Select(word =>
        word.Length == 0 ? word : char.ToUpperInvariant(word[0]) + word[1..]));

    private static Dictionary<string, Model> LoadModels(string root)
    {
        var models = new Dictionary<string, Model>(StringComparer.Ordinal);
        foreach (string source in Sources)
        {
            string text = Regex.Replace(ReadText(Path.Combine(root, source)), @"//[^\n]*", "");
            const string pattern = @"((?:\s*#\[[^\]]*\])*)\s*pub (struct|enum) (\w+)(?:<[^>{}]*>)?\s*\{";
            foreach (Match match in Regex.Matches(text, pattern))
            {
                int start = match.Index + match.Length, depth = 1, end = start;
                while (depth != 0 && end < text.Length)
                {
                    if (text[end] == '{') depth++;
                    if (text[end] == '}') depth--;
                    end++;
                }
                if (depth != 0) throw new InvalidOperationException($"unclosed model: {match.Groups[3].Value}");
                models[match.Groups[3].Value] = new(match.Groups[2].Value, match.Groups[1].Value, text[start..(end - 1)], source);
            }
        }
        return models;
    }

    internal static string Generate(string root)
    {
        var models = LoadModels(root);
        var pending = new Queue<string>(RootTypes);
        var emitted = new SortedDictionary<string, string>(StringComparer.Ordinal);

        string CsType(string rust)
        {
            rust = Regex.Replace(rust.Trim(), @"&\s*(?:'\w+\s*)?", "");
            if (rust.StartsWith("Option<", StringComparison.Ordinal) && rust.EndsWith('>'))
                return CsType(rust[7..^1]) + "?";
            // List<byte> preserves Rust's JSON number array; byte[] would serialize as base64.
            if (rust.StartsWith("Vec<", StringComparison.Ordinal) && rust.EndsWith('>'))
                return "List<" + CsType(rust[4..^1]) + ">";
            if (rust.StartsWith("BTreeMap<", StringComparison.Ordinal) && rust.EndsWith('>'))
                return "Dictionary<" + string.Join(", ", SplitTop(rust[9..^1]).Select(CsType)) + ">";
            if (Scalars.TryGetValue(rust, out var scalar)) return scalar;
            if (Custom.TryGetValue(rust, out var mapped))
            {
                if (models.ContainsKey(mapped)) pending.Enqueue(mapped);
                return mapped;
            }
            if (!models.ContainsKey(rust)) throw new InvalidOperationException($"unsupported Rust type: {rust}");
            pending.Enqueue(rust);
            return rust;
        }

        while (pending.TryDequeue(out var name))
        {
            if (emitted.ContainsKey(name)) continue;
            var model = models[name];
            var attributes = Attributes(model.Attributes);
            var lines = new List<string> { $"// Source: {model.Source} :: {name}" };
            if (model.Kind == "enum")
            {
                lines.AddRange([$"[JsonConverter(typeof({name}Converter))]", $"public enum {name}", "{"]);
                foreach (string item in SplitTop(model.Body))
                {
                    string raw = Regex.Replace(item, @"#\[[^\]]*\]", "").Trim();
                    if (!Regex.IsMatch(raw, @"\A\w+\z"))
                        throw new InvalidOperationException($"unsupported data enum: {name}: {raw}");
                    string wire = Attributes(item).GetValueOrDefault("rename") ?? Rename(raw, attributes.GetValueOrDefault("rename_all"));
                    lines.AddRange([$"    [JsonStringEnumMemberName(\"{wire}\")]", $"    {raw},"]);
                }
                lines.AddRange(["}", $"internal sealed class {name}Converter() : JsonStringEnumConverter<{name}>(allowIntegerValues: false);"]);
            }
            else
            {
                lines.AddRange([$"public class {name}", "{"]);
                foreach (string item in SplitTop(model.Body))
                {
                    string raw = Regex.Replace(item, @"#\[[^\]]*\]", "").Trim();
                    var field = Regex.Match(raw, @"\Apub (\w+)\s*:\s*(.+)\z", RegexOptions.Singleline);
                    if (!field.Success) throw new InvalidOperationException($"unsupported field: {name}: {raw}");
                    string fieldName = field.Groups[1].Value;
                    var fieldAttributes = Attributes(item);
                    string wire = fieldAttributes.GetValueOrDefault("rename") ?? Rename(fieldName, attributes.GetValueOrDefault("rename_all"));
                    string mapped = CsType(field.Groups[2].Value);
                    bool useDefault = attributes.ContainsKey("default") || fieldAttributes.ContainsKey("default");
                    bool required = !mapped.EndsWith('?') && !useDefault;
                    string init = "";
                    if (useDefault && !mapped.EndsWith('?'))
                    {
                        if (mapped.StartsWith("List<", StringComparison.Ordinal) || mapped.StartsWith("Dictionary<", StringComparison.Ordinal)) init = " = [];";
                        else if (mapped is not ("int" or "uint" or "ulong" or "byte" or "float" or "double" or "bool" or "JsonElement"))
                            init = mapped == "string" ? " = \"\";" : " = new();";
                    }
                    lines.AddRange([$"    [JsonPropertyName(\"{wire}\")]",
                        $"    public {(required ? "required " : "")}{mapped} {Pascal(fieldName)} {{ get; init; }}{init}"]);
                }
                lines.Add("}");
            }
            emitted[name] = string.Join('\n', lines);
        }
        string header = $"// <auto-generated />\n// Run: {Program.GenerateCommand}\n#nullable enable\nusing System.Text.Json;\nusing System.Text.Json.Serialization;\n\nnamespace ShojiWM.Wire;\n\n";
        return header + string.Join("\n\n", emitted.Values) + "\n";
    }

    // Python read_text used universal newlines. Keep parsing identical on CRLF checkouts.
    private static string ReadText(string path) => File.ReadAllText(path, Utf8).Replace("\r\n", "\n").Replace('\r', '\n');

    internal static void Write(string root)
    {
        string result = Generate(root); // Validate the entire closure before touching the output.
        string output = Path.Combine(root, Output);
        Directory.CreateDirectory(Path.GetDirectoryName(output)!);
        if (!File.Exists(output) || !File.ReadAllBytes(output).AsSpan().SequenceEqual(Utf8.GetBytes(result)))
            File.WriteAllText(output, result, Utf8);
    }

    internal static void Check(string root)
    {
        string output = Path.Combine(root, Output);
        if (!File.Exists(output) || !File.ReadAllBytes(output).AsSpan().SequenceEqual(Utf8.GetBytes(Generate(root))))
            throw new InvalidOperationException($"Generated bindings are stale; run {Program.GenerateCommand}");
    }
}

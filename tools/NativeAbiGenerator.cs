using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace ShojiWM.Tools;

// Reviewed local manifest, independent of upstream layout and serde annotations.
internal static class NativeAbiGenerator
{
    internal sealed record Field(string Name, string Rust, string Type);
    internal sealed record Model(string Name, string Semantic, Field[] Fields);
    internal sealed record EnumModel(string Name, string Semantic, string[] Values);
    internal sealed record Schema(Model[] Models, EnumModel[] Enums);
    private static string FormatRust(string source)
    {
        var start = new ProcessStartInfo("rustfmt") { RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false };
        start.ArgumentList.Add("--edition"); start.ArgumentList.Add("2024"); start.ArgumentList.Add("--emit"); start.ArgumentList.Add("stdout");
        using var process = Process.Start(start) ?? throw new InvalidOperationException("Cannot start rustfmt");
        process.StandardInput.Write(source); process.StandardInput.Close();
        string output = process.StandardOutput.ReadToEnd(), error = process.StandardError.ReadToEnd();
        process.WaitForExit();
        if (process.ExitCode != 0) throw new InvalidOperationException("native ABI rustfmt: " + error);
        return output.Replace("\r\n", "\n");
    }
    internal static void Run(string root, bool check)
    {
        var schema = JsonSerializer.Deserialize<Schema>(File.ReadAllText(Path.Combine(root, "tools/NativeAbi.schema.json")), new JsonSerializerOptions { PropertyNameCaseInsensitive = true })!;
        var enums = schema.Enums.Select(x => x.Name).ToHashSet();
        var slices = new HashSet<string> { "string", "WaylandOutputEntry", "RuntimeInputEntry", "RuntimeWindowAction" };
        string Base(string t) => t.TrimEnd('?');
        bool Optional(string t) => t.EndsWith('?');
        bool List(string t) => Base(t).StartsWith("List<");
        string Item(string t) => Base(t)[5..^1];
        string Label(string t) => t == "string" ? "String" : t == "byte" ? "Byte" : t;
        void ValidateType(string t)
        {
            if (Optional(t)) { ValidateType(Base(t)); return; }
            if (List(t)) { ValidateType(Item(t)); return; }
            if (t is "string" or "bool" or "int" or "uint" or "ulong" or "byte" or "float" or "double" or "JsonElement" or "Dimension" or "FontFamily" or "ClickDescriptor" or "ResizeHitArea" || enums.Contains(t) || schema.Models.Any(x => x.Name == t)) return;
            throw new InvalidOperationException("unsupported ABI type: " + t);
        }
        foreach (var model in schema.Models) foreach (var field in model.Fields) ValidateType(field.Type);
        string Cs(string t)
        {
            if (Optional(t)) return Cs(Base(t)) + "*";
            if (List(t)) { slices.Add(Item(t)); return Label(Item(t)) + "Slice"; }
            if (enums.Contains(t)) return "uint";
            return t switch { "string" => "ArenaString", "bool" => "byte", "int" or "uint" or "ulong" or "byte" or "float" or "double" => t, _ => "Abi" + t };
        }
        string Rs(string t)
        {
            if (Optional(t)) return "*const " + Rs(Base(t));
            if (List(t)) return "Slice<" + Rs(Item(t)) + ">";
            if (enums.Contains(t)) return "u32";
            return t switch { "string" => "ArenaString", "bool" or "byte" => "u8", "int" => "i32", "uint" => "u32", "ulong" => "u64", "float" => "f32", "double" => "f64", _ => "Abi" + t };
        }
        string CWrite(string t, string v)
        {
            if (Optional(t)) return $"{v} is {{ }} item{v.Replace(".", "")} ? w.Put({CWrite(Base(t), "item" + v.Replace(".", ""))}) : null";
            if (List(t)) return $"Write{Label(Item(t))}Slice({v}, w, depth + 1)";
            if (enums.Contains(t)) return $"Write{t}({v})";
            return t switch { "string" => $"w.String({v})", "bool" => $"(byte)({v} ? 1 : 0)", "float" or "double" => $"Finite({v})", "int" or "uint" or "ulong" or "byte" => v, _ => $"Write{t}({v}, w, depth + 1)" };
        }
        string CRead(string t, string v)
        {
            if (Optional(t)) return $"{v} == null ? ({t})null : {CRead(Base(t), "*" + v)}";
            if (List(t)) return $"Read{Label(Item(t))}Slice({v}, depth + 1)";
            if (enums.Contains(t)) return $"Read{t}({v})";
            return t switch { "string" => $"ReadString({v})", "bool" => $"ReadBool({v})", "float" or "double" => $"Finite({v})", "int" or "uint" or "ulong" or "byte" => v, _ => $"Read{t}({v}, depth + 1)" };
        }
        string RRead(string t, string v)
        {
            if (Optional(t)) return $"r.optional({v}, |v, r, depth| Ok({RRead(Base(t), "v")} ), depth + 1)?";
            if (List(t)) return $"r.array({v}, |v, r, depth| Ok({RRead(Item(t), "v")}), depth + 1)?";
            if (enums.Contains(t)) return $"decode_{t}({v})?";
            return t switch { "string" => $"r.string({v})?", "bool" => $"decode_bool({v})?", "float" => $"decode_f32({v})?", "double" => $"decode_f64({v})?", "int" or "uint" or "ulong" or "byte" => v, _ => $"decode_{t}({v}, r, depth + 1)?" };
        }
        string RWrite(string t, string v)
        {
            if (Optional(t)) return $"w.optional({v}.as_ref(), |v, w| Ok({RWrite(Base(t), "v")}))?";
            if (List(t)) return $"w.array({v}, |v, w| Ok({RWrite(Item(t), "v")}))?";
            if (enums.Contains(t)) return $"encode_{t}({v})";
            return t switch { "string" => $"w.string({v})?", "bool" => $"u8::from(*{v})", "int" or "uint" or "ulong" or "byte" or "float" or "double" => "*" + v, _ => $"encode_{t}({v}, w)?" };
        }
        var cs = new StringBuilder("// <auto-generated />\n#nullable enable\nusing System.Runtime.InteropServices;\nusing ShojiWM.Wire;\nnamespace ShojiWM.Runtime;\n");
        var rs = new StringBuilder("// Generated from tools/NativeAbi.schema.json. Do not edit.\n#![allow(non_snake_case, dead_code, unused_variables, unused_parens)]\nuse super::arena::*;\n");
        foreach (var m in schema.Models)
        {
            var fields = m.Fields.Where(f => Base(f.Type) != "JsonElement").ToArray();
            cs.AppendLine($"[StructLayout(LayoutKind.Sequential)] public unsafe struct Abi{m.Name} {{");
            rs.AppendLine($"#[repr(C)] #[derive(Clone, Copy)] pub struct Abi{m.Name} {{");
            foreach (var f in fields) { cs.AppendLine($"public {Cs(f.Type)} {f.Name};"); rs.AppendLine($"pub {f.Rust}: {Rs(f.Type)},"); }
            cs.AppendLine("}"); rs.AppendLine("}");
            rs.AppendLine($"pub fn decode_{m.Name}(v: Abi{m.Name}, r: &mut Reader, depth: usize) -> Result<{m.Semantic}, String> {{ r.depth(depth)?; Ok({m.Semantic} {{");
            foreach (var f in m.Fields)
            {
                string expr = Base(f.Type) == "JsonElement" ? "None" : RRead(f.Type, "v." + f.Rust);
                if (m.Name == "WireDecorationNode" && f.Name == "Children") expr = "r.array(v.children, |v, r, depth| decode_WireDecorationNode(v, r, depth).map(shojiwm_lib::ssd::bridge::WireDecorationChild::Node), depth + 1)?";
                rs.AppendLine($"{f.Rust}: {expr},");
            }
            rs.AppendLine("}) }");
            // Result-only models aren't encoded by Rust. No semantic foreign memory escapes.
            if (!m.Name.StartsWith("Wire") && m.Name != "RuntimeWindowAction")
            {
                rs.AppendLine($"pub fn encode_{m.Name}(v: &{m.Semantic}, w: &mut Writer) -> Result<Abi{m.Name}, String> {{ Ok(Abi{m.Name} {{");
                foreach (var f in fields) rs.AppendLine($"{f.Rust}: {RWrite(f.Type, "(&v." + f.Rust + ")")},");
                rs.AppendLine("}) }");
            }
        }
        cs.AppendLine("public static unsafe partial class AbiConvert {");
        foreach (var m in schema.Models)
        {
            cs.AppendLine($"internal static Abi{m.Name} Write{m.Name}({m.Name} v, ArenaWriter w, int depth) {{ CheckDepth(depth);");
            foreach (var f in m.Fields.Where(f => Base(f.Type) == "JsonElement")) cs.AppendLine($"if (v.{f.Name} is not null) throw new NotSupportedException(\"{m.Name}.{f.Name} is unsupported by native ABI\");");
            cs.AppendLine($"return new Abi{m.Name} {{");
            foreach (var f in m.Fields.Where(f => Base(f.Type) != "JsonElement")) cs.AppendLine($"{f.Name} = {CWrite(f.Type, "v." + f.Name)},");
            cs.AppendLine("}; }");
            cs.AppendLine($"internal static {m.Name} Read{m.Name}(Abi{m.Name} v, int depth) {{ CheckDepth(depth); return new {m.Name} {{");
            foreach (var f in m.Fields.Where(f => Base(f.Type) != "JsonElement")) cs.AppendLine($"{f.Name} = {CRead(f.Type, "v." + f.Name)},");
            cs.AppendLine("}; }");
        }
        foreach (var e in schema.Enums)
        {
            cs.AppendLine($"static uint Write{e.Name}({e.Name} v) => v switch {{");
            rs.AppendLine($"fn decode_{e.Name}(v: u32) -> Result<{e.Semantic}, String> {{ match v {{");
            for (int i = 0; i < e.Values.Length; i++) { cs.AppendLine($"{e.Name}.{e.Values[i]} => {i},"); rs.AppendLine($"{i} => Ok({e.Semantic}::{e.Values[i]}),"); }
            cs.AppendLine("_ => throw new ArgumentException(\"invalid enum tag\") }; "); rs.AppendLine("_ => Err(\"invalid enum tag\".into()), } }");
            cs.AppendLine($"static {e.Name} Read{e.Name}(uint v) => v switch {{");
            rs.AppendLine($"fn encode_{e.Name}(v: &{e.Semantic}) -> u32 {{ match v {{");
            for (int i = 0; i < e.Values.Length; i++) { cs.AppendLine($"{i} => {e.Name}.{e.Values[i]},"); rs.AppendLine($"{e.Semantic}::{e.Values[i]} => {i},"); }
            cs.AppendLine("_ => throw new ArgumentException(\"invalid enum tag\") }; "); rs.AppendLine("} }");
        }
        foreach (string item in slices.Order(StringComparer.Ordinal))
        {
            string label = Label(item), type = Cs(item);
            cs.AppendLine($"internal static {label}Slice Write{label}Slice(IReadOnlyList<{item}> values, ArenaWriter w, int depth) {{ CheckDepth(depth); int count = values.Count; var ptr = w.Allocate<{type}>(count); for (int i = 0; i < count; i++) {{ var value = {CWrite(item, "values[i]")}; if (!w.Measuring) ptr[i] = value; }} return new() {{ Ptr = ptr, Length = count }}; }}");
            cs.AppendLine($"internal static List<{item}> Read{label}Slice({label}Slice v, int depth) {{ CheckDepth(depth); CheckSlice(v.Ptr, v.Length); var result = new List<{item}>(v.Length); for(int i=0;i<v.Length;i++) result.Add({CRead(item, "v.Ptr[i]")}); return result; }}");
        }
        cs.AppendLine("}");
        foreach (string item in slices.Order(StringComparer.Ordinal)) cs.AppendLine($"[StructLayout(LayoutKind.Sequential)] public unsafe struct {Label(item)}Slice {{ public {Cs(item)}* Ptr; public int Length; }}");
        (int Size, int Align) Layout(string t)
        {
            if (Optional(t) || t == "ulong" || t == "double") return (8, 8);
            if (List(t) || t == "string") return (16, 8);
            if (t == "bool" || t == "byte") return (1, 1);
            if (t is "int" or "uint" or "float" || enums.Contains(t)) return (4, 4);
            if (t == "Dimension") return (24, 8);
            if (t == "FontFamily" || t == "ResizeHitArea") return (16, 8);
            if (t == "ClickDescriptor") return (24, 8);
            var model = schema.Models.Single(x => x.Name == t);
            int size = 0, align = 1;
            foreach (var f in model.Fields.Where(f => Base(f.Type) != "JsonElement")) { var l = Layout(f.Type); size = (size + l.Align - 1) / l.Align * l.Align + l.Size; align = Math.Max(align, l.Align); }
            return ((size + align - 1) / align * align, align);
        }
        var ctest = new StringBuilder("// <auto-generated />\nusing System.Runtime.InteropServices;\nnamespace ShojiWM.Runtime;\npublic static unsafe class NativeLayouts { public static void Validate() { if (IntPtr.Size != 8) throw new PlatformNotSupportedException();\n");
        rs.AppendLine("#[cfg(test)] mod layout_tests { use super::*; #[test] fn manifest_layouts() { assert_eq!(std::mem::size_of::<usize>(), 8);");
        foreach (var m in schema.Models) {
            var l = Layout(m.Name);
            ctest.AppendLine($"Check(sizeof(Abi{m.Name}) == {l.Size}); Check(Marshal.OffsetOf<Align{m.Name}>(nameof(Align{m.Name}.Value)) == {l.Align});");
            rs.AppendLine($"assert_eq!(std::mem::size_of::<Abi{m.Name}>(), {l.Size}); assert_eq!(std::mem::align_of::<Abi{m.Name}>(), {l.Align});");
            int offset = 0;
            foreach (var f in m.Fields.Where(f => Base(f.Type) != "JsonElement")) {
                var fl = Layout(f.Type); offset = (offset + fl.Align - 1) / fl.Align * fl.Align;
                ctest.AppendLine($"Check(Marshal.OffsetOf<Abi{m.Name}>(nameof(Abi{m.Name}.{f.Name})) == {offset});");
                rs.AppendLine($"assert_eq!(std::mem::offset_of!(Abi{m.Name}, {f.Rust}), {offset});");
                offset += fl.Size;
            }
        }
        foreach (string item in slices.Order(StringComparer.Ordinal)) ctest.AppendLine($"Check(sizeof({Label(item)}Slice) == 16); Check(Marshal.OffsetOf<{Label(item)}Slice>(nameof({Label(item)}Slice.Ptr)) == 0); Check(Marshal.OffsetOf<{Label(item)}Slice>(nameof({Label(item)}Slice.Length)) == 8);");
        ctest.AppendLine("} static void Check(bool v) { if (!v) throw new InvalidOperationException(\"native ABI layout mismatch\"); } }");
        foreach (var m in schema.Models) ctest.AppendLine($"[StructLayout(LayoutKind.Sequential)] internal struct Align{m.Name} {{ public byte Prefix; public Abi{m.Name} Value; }}");
        rs.AppendLine("} }");
        string rustOutput = FormatRust(rs.ToString());
        Save("dotnet/ShojiWM.Runtime/Ffi/Generated/NativeLayouts.g.cs", ctest.ToString());
        Save("dotnet/ShojiWM.Runtime/Ffi/Generated/NativeAbi.g.cs", cs.ToString());
        Save("src/bridge/native_generated.rs", rustOutput);
        void Save(string file, string text)
        {
            string path = Path.Combine(root, file);
            if (check) { if (!File.Exists(path) || File.ReadAllText(path) != text) throw new InvalidOperationException("Native ABI bindings are stale: " + file); }
            else { Directory.CreateDirectory(Path.GetDirectoryName(path)!); if (!File.Exists(path) || File.ReadAllText(path) != text) File.WriteAllText(path, text, new UTF8Encoding(false)); }
        }
    }
}

using System.CommandLine;
using System.CommandLine.Help;

namespace ShojiWM.Tools;

internal static class Program
{
    internal const string GenerateCommand = "dotnet run --project tools/ShojiWM.Tools.csproj -- generate";

    private static int Main(string[] args)
    {
        try
        {
            return CreateCommand().Parse(args).Invoke(new InvocationConfiguration { EnableDefaultExceptionHandler = false });
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception.Message);
            return 1;
        }
    }

    internal static RootCommand CreateCommand()
    {
        var check = new Option<bool>("--check")
        {
            Description = "Check generated bindings without modifying them.",
        };
        var generate = new Command("generate", "Generate bindings and native FFI boilerplate.") { check };
        generate.SetAction(result =>
        {
            string root = FindRoot();
            if (result.GetValue(check)) BindingGenerator.Check(root);
            else BindingGenerator.Write(root);
            NativeAbiGenerator.Run(root, result.GetValue(check));
            NativeOperationGenerator.Run(root, result.GetValue(check));
        });

        var test = new Command("test", "Run binding generator regression tests.");
        test.SetAction(_ => GeneratorTests.Run(FindRoot()));

        var validate = new Command("validate", "Run builds, tests and runtime integration validation.");
        validate.SetAction(_ => Validation.Run(FindRoot()));

        var root = new RootCommand("ShojiWM .NET binding generation and validation tools.") { generate, test, validate };
        // Show help for an empty invocation, while retaining errors for unknown arguments.
        root.SetAction(result => new HelpAction().Invoke(result));
        return root;
    }

    // Resolve from the executable, so generation also works outside the repository cwd.
    private static string FindRoot()
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
            if (File.Exists(Path.Combine(directory.FullName, "tools/NativeAbi.schema.json")) &&
                File.Exists(Path.Combine(directory.FullName, "Cargo.toml")))
                return directory.FullName;
        throw new InvalidOperationException("Cannot find the repository root from the tool location.");
    }
}

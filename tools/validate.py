#!/usr/bin/env python3
"""Build and verify the independent runtime; never format upstream files."""
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
os.chdir(ROOT)


def upstream_clean():
    status = subprocess.check_output(["git", "-C", "ShojiWM", "status", "--porcelain"], text=True)
    if status:
        raise RuntimeError("ShojiWM must remain unchanged:\n" + status)


def run(*args, env=None):
    subprocess.run(args, check=True, env=env)


upstream_clean()
try:
    run("rustfmt", "--check", "--edition", "2024", "src/lib.rs", "src/main.rs", "tests/runtime.rs")
    run("python3", "tools/generate-dotnet-bindings.py")
    run("python3", "tools/generate-dotnet-bindings.py", "--check")
    run("python3", "tools/test-dotnet-generator.py")
    run("cargo", "build", "--locked", "--offline")
    run("cargo", "test", "--locked", "--offline")
    run("dotnet", "build", "dotnet/ShojiWM.Tests/ShojiWM.Tests.csproj", "-c", "Release",
        "--disable-build-servers", "-m:1")
    base = ROOT / "dotnet"
    runtime = base / "ShojiWM.Runtime/bin/Release/net10.0/ShojiWM.Runtime.dll"
    fixture = base / "ShojiWM.Tests/bin/Release/net10.0/ShojiWM.Tests.dll"
    example = base / "ShojiWM.Example/bin/Release/net10.0/ShojiWM.Example.dll"
    run("dotnet", str(fixture))
    env = os.environ.copy()
    env.update(SHOJI_TEST_DOTNET_RUNTIME=str(runtime), SHOJI_TEST_DOTNET_CONFIG=str(example),
               SHOJI_TEST_DOTNET_FIXTURE=str(fixture))
    run("cargo", "test", "--locked", "--offline", "--", "--ignored", "--test-threads=1", env=env)
    run("cargo", "run", "--locked", "--offline", "--manifest-path", "dotnet/NativeHost.Tests/Cargo.toml",
        "--", str(runtime), str(fixture))
    run("target/debug/ShojiWM-dotnet", "--help")
    run("target/debug/ShojiWM-dotnet", "--version")
finally:
    upstream_clean()

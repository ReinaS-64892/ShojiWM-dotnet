# 作業規則

- 作業報告とドキュメントは、原則として簡潔な日本語で書く。
- `README.md` に利用・実装メモを追記しない。今後の補足説明は `AI-UsageNote.md` に追記する。
- 既存の未コミット変更は、依頼された範囲を除いて変更・破棄・コミットしない。
- `ShojiWM/` は upstream submodule として扱い、ソースや submodule の参照コミットを変更しない。
- `old-integrated/ShojiWM/` は旧実装の参照専用。旧 compositor patch を再導入しない。
- .NET 固有実装は外側のリポジトリに置く。接続には公開 API の `RuntimeLauncher`、`ConfigRuntime`、`RuntimeHost` を使う。
- 公開 API で表現できない機能は upstream API gap として記録し、upstream を変更して解決しない。
- 既存の hostfxr / C ABI を活用し、reload・unload・shutdown 時の所有権と callback の寿命を守る。
- 移植履歴・過去の検証記録は `old-integrated/migration-notes/` に置く。この場所は Git の追跡対象外なので、保存とコミットを区別する。

## 検証

- バインディング生成は `dotnet run --project tools/ShojiWM.Tools.csproj -- generate`。末尾に `--check` を付けると、生成物を書き換えずに確認できる。
- 生成器の回帰テストは `dotnet run --project tools/ShojiWM.Tools.csproj -- test`。Python は不要。CLI の引数処理には `System.CommandLine` を使う。
- 変更に応じた build / test を行う。全体の検証は `dotnet run --project tools/ShojiWM.Tools.csproj -- validate` を使う。
- `cargo fmt --all` は path dependency の upstream も整形するため使わない。外側のファイルを明示して `rustfmt` を実行する。
- 完了時は `git diff --check` と `git -C ShojiWM status --porcelain` を確認する。upstream の status は空であること。
- 実行できなかった検証や既知の制約は、報告に明記する。

## コミット

- ユーザーからコミットを依頼された場合に行う。依頼範囲の変更だけを stage し、差分を確認する。
- コミットメッセージは `feat:`、`fix:`、`docs:`、`chore:` などの接頭辞を付け、変更内容を短く英語で書く。
- Author と Committer はともに `OpenAI Codex <codex@openai.com>` とする。リポジトリやグローバルの Git 設定は変更しない。

```bash
git -c user.name='OpenAI Codex' -c user.email='codex@openai.com' \
  -c commit.gpgSign=false commit \
  --author='OpenAI Codex <codex@openai.com>' \
  -m 'docs: describe repository rules'
```

- 完了報告にはコミット ID と検証結果を記載する。push は依頼された場合に行う。

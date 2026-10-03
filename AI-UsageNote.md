# AI による利用・実装メモ

## 非同期 reload

設定プロジェクトを `--dotnet-project=/path/to/Config.csproj`、生成される assembly を
`--config=/path/to/Config.dll` で指定すると、起動時と reload 時に C# をコンパイルします。

`Super+Shift+R` による reload は upstream の非同期 API を使います。
コンパイルと DLL の退避はバックグラウンドで進み、その間も旧設定が応答します。
完了を `RuntimeHost` の `ReloadReady` で通知し、compositor の反映時点で新 assembly を
ロード・検証・切り替えます。失敗時は旧設定を維持し、再度 reload できます。
準備中の reload 操作は一つにまとめ、終了時はビルドをキャンセルして worker を join します。

初回起動の `preload()` は同期処理です。managed configuration の初期化・検証・切り替えも
反映時点の同期処理ですが、reload の C# コンパイルは compositor thread で実行しません。

## Native transport ABI v6

Rust ↔ .NET の実行時通信は、操作別 `SWM*` unmanaged export に移行しました。
JSON invoke、requestId、ExternalRuntimeRequest/Response は廃止しています。
公開設定モデルの JSON converter とテスト fixture は引き続き別用途で使用します。
upstream と generic runtime injection の境界は変更していません。

各 export は `ConfigurationHost` → `RuntimeSession` の対応する typed method を
直接呼びます。operation enum、汎用 Handle、request/response union は使用しません。
Rust も名前付き function pointer を直接呼び、channel は Rust-owned データを持つ
job の転送だけを担います。共通 helper は thread・例外・arena の処理に限定しています。
v5 で operation 別 input/result の signature を変更しました。v6 では例外診断と
NULL failure arena の契約を更新したため、旧 version とは非互換です。

ABI は 64-bit pointer を前提とし、hostfxr の delegate 取得前に version を照合します。
`tools/NativeAbi.schema.json` は外側リポジトリが所有する明示的な ABI manifest です。
既存の `generate` / `generate --check` が C#・Rust DTO と共通の layout 検証を生成します。
生成には .NET SDK と `rustfmt` が必要です。upstream DTO の layout は ABI に使用しません。
`tools/NativeOperationGenerator.cs` の明示的な定義から、operation 別 struct、Read/Write、
直接呼び出す entry point を生成します。`generate` / `generate --check` の対象です。
managed FFI は `dotnet/ShojiWM.Runtime/Ffi/`、生成物はその `Generated/` 配下です。
`Generated/` と `src/bridge/native_generated.rs` は Git 追跡対象外です。clean checkout では
Rust build の前に `dotnet run --project tools/ShojiWM.Tools.csproj -- generate` を実行します。
`validate` は生成から行います。.NET build でも project dependency が生成器を実行します。
arena・特殊 scalar の変換・host の所有権管理は手書きで維持します。
Rust 側 operation root は `native.rs`、補助型は `arena.rs` に定義しています。

入力は Rust-owned request を channel で runtime thread に移動し、その thread で作った
一時 arena の borrowed view として渡します。.NET は入力を managed 値にコピーします。
結果はサイズ計算と書き込みの二回走査で、8-byte alignment の単一 contiguous allocation に
配置します。文字列は UTF-8 pointer + byte length、nullable は NULL、bool は 0/1 byte、
enum は検証付き u32 tag です。managed pointer、pin、結果用 GCHandle は使用しません。
Rust は範囲・alignment・負の length・overflow・UTF-8・tag・深度を検証し、owned 値に
変換します。RAII が成功・失敗とも `SWMArenaFree` を一回呼び、foreign pointer は
runtime thread の外へ出ません。allocator が返す arena base 自体の allocation provenance は
FFI 契約に依存します。範囲検査は任意の虚偽 base address の可読性まで保証しません。

status は 0=成功、1=設定/runtime semantic failure、2=host failure、4=invalid input です。
例外型による status の判定はせず、処理段階の分類のみを維持します。
managed 側の分類は `NativeFailureStatus` enum で扱い、ABI の `Status` は `int` に変換します。
各 export は一つの例外境界で `Exception.ToString()` を返します。診断は最大4096文字です。
診断 arena も構築できない failure は元の status と NULL arena を返します。
payload を返さない `SwmStatusResult` の成功は `status=0, arena={NULL,0}, error=NULL`
です。managed/native の allocation はなく、Rust はこの正規形で `SWMArenaFree` を呼びません。
payload を持つ成功と error arena は従来どおり検証・decode 後に一回 free します。
Rust は診断が取得できなかった旨を補い、元の分類を保持します。
semantic failure は host を停止せず、host/ABI failure は従来どおり restart required として扱います。
入力・結果 arena は最大 8 MiB、浮動小数点の NaN/Infinity は拒否します。native graph は最大 depth 64、managed composition は
最大 depth 30 です。構築途中の失敗でも builder が未公開 allocation を解放します。

未実装の `WireProps.Icon` / `Shader`、`WireStyle.FontWeight`、
`RuntimeWindowAction.Animation` は値が指定された場合に明示的 unsupported error とします。
ABI に JSON blob を埋め込む経路はありません。対応拡張時は native representation と
manifest を更新し、互換性のない layout 変更時は双方の ABI version を更新してください。
reload の candidate/commit/abort、delegate ID の generation 分離、collectible ALC unload の
検証は維持しています。arena は generation に依存しない native 値のみを保持します。

比較測定は次で実行できます。

```bash
dotnet run --project dotnet/ShojiWM.Tests/ShojiWM.Tests.csproj -c Release -- --measure
```

テスト専用の旧 JSON transport oracle と native transport の response cost、および
managed evaluation を含む往復を測定します。managed allocation bytes、native allocation
回数・bytes、平均 latency を出力します。compositor/Rust decoder を含む end-to-end benchmark
ではありません。1 / 128 / 512 nodes・strings・actions を比較し、CI に速度閾値は設けません。
native response allocation が一回であることは回帰テストでも検証します。
数値は JIT・負荷・環境に依存します。

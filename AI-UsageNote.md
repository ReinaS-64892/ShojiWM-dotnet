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

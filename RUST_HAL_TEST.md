# Rust HAL実機試験

開発用の `ChannelBridgeRustTest.driver` は、通常版と異なるUID・バンドルID・ファイル名を使う。現行ドライバ、既定の入出力、既存設定を置き換えない。

## ビルド・導入

```sh
bash scripts/test-rust.sh
bash scripts/package-rust-test-driver.sh
open work/rust-dist/ChannelBridgeRustTest.pkg
```

インストーラでmacOSの管理者認証が必要。配置先は `/Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driver`。パッケージには音声サービスを再起動するスクリプトを含めない。導入直後に認識されなければ、利用者が通話等を終えた後にMacを再起動してから確認する。

このパッケージは開発用ad-hoc署名のドライバを含む。一般配布向けのDeveloper ID署名・公証は未検証。

## 列挙・合成音の転送

以下は同じBash環境で実行する。

```sh
source scripts/rust-env.sh
cargo run -p channelbridge-macos --example probe
cargo run -p channelbridge-macos --example probe -- --loopback
```

列挙は読み取りのみ。`--loopback` はUID `ChannelBridge_Rust_Test_UID` のデバイスだけを選択し、2ch float32形式を検証して3秒間の997 Hz合成音を仮想出力へ送り、同じデバイスの入力から読み戻す。物理マイクは開始せず、音声ファイルを作らない。

合成音の左右は異なる位相・振幅を持つ。入力時刻から期待波形を計算し、破損・左右の入れ替え・開始後の無音化を数える。初期の未書込区間だけは別集計する。10回以上のコールバック、4096以上の比較フレーム、破損・不正コールバック・音声開始後の欠落が0で短時間試験を合格とする。

この試験は時計差補正、物理入力、Discord、長時間安定性を証明しない。初期のロード・形式・時刻・転送・停止の確認に使う。

## 実HALのレート変更

試験アプリを終了してから、同じBash環境で次を実行する。

```sh
cargo run --locked -p channelbridge-macos --example rate_loopback
```

試験UIDだけを44.1/48/88.2/96/176.4/192/48 kHzへ順に変更し、各段階で入力・出力双方の形式が追従してから3秒の合成音転送を確認する。通常終了と試験エラー時には開始前の試験デバイスのレートへ戻す処理を試み、復元失敗もエラーとして表示する。プロセス強制終了時の復元は保証しない。物理入力や既定機器を変更しない。これは停止中の非同期レート変更の試験であり、送出中の物理機器変更に対する自動復旧や全対応レートの合格ではない。

## 仮想マイクの読み取りだけを行う確認

```sh
cargo run --locked -p channelbridge-macos --example monitor -- --expect-silence
cargo run --locked -p channelbridge-macos --example monitor -- --expect-signal
```

試験アプリと併用できる3秒の読み取り専用検査。試験UIDの出力ストリームをこの読み手について無効化し、レートや既定機器を変えず、別の書き手の音声を妨げない。既にOSがマイクアクセスを許可している実行元でのみ開始し、許可を要求する画面は出さない。サンプルを保存せず、左右別の非ゼロフレーム数、絶対ピーク、非有限サンプル、不正callbackを集計する。信号ありは少なくとも片側が非ゼロ、無音は両側がすべて厳密なゼロである。無音の原因、波形の一致、ドロップアウト、周波数特性、左右の対応、遅延や通話品質はこの集計だけでは判定できない。

送出停止・ミュート時は無音、実入力に音がある送出時は信号ありを確認する。起動/ミュート切り替えの短い過渡区間が測定へ入らないよう、画面の状態反映後に各検査を開始する。引数なしなら値だけを報告する。どのモードでも不正callback/サンプルまたは検査量不足は失敗となる。

## 削除

通話等を終え、試験を停止してから試験ドライバだけを取り除く。

```sh
sudo rm -rf /Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driver
sudo pkgutil --forget local.channelbridge.rust.driver.test.pkg
```

削除後の認識更新が必要ならMacを再起動する。通常版 `ChannelBridge.driver` は削除しない。

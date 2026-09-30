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

## 削除

通話等を終え、試験を停止してから試験ドライバだけを取り除く。

```sh
sudo rm -rf /Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driver
sudo pkgutil --forget local.channelbridge.rust.driver.test.pkg
```

削除後の認識更新が必要ならMacを再起動する。通常版 `ChannelBridge.driver` は削除しない。

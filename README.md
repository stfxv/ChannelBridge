# ChannelBridge

多チャンネルのオーディオインターフェースから必要な入力を選び、2チャンネルの仮想マイク **ChannelBridge Mic** へ送るmacOSアプリである。正式版はRustで実装したアプリと独自HALドライバを使用し、BlackHoleやLadioCastを必要としない。

## 導入

`dist/ChannelBridge-Rust.pkg`を開いてアプリとドライバを導入し、Macを再起動する。アプリは`/Applications/ChannelBridge.app`に配置される。旧通常版と同じ機器UID・設定先を使用する。導入前のバックアップと復帰手順は[RUST_MIGRATION.md](RUST_MIGRATION.md)を参照する。

入力機器・入力チャンネル・mono/stereo/mixを選び、マイク使用を許可する。送出開始前から入力レベルを確認できる。通話アプリ側は **ChannelBridge Mic** を入力に選ぶ。選択した入力のdBFS、物理入力形式、仮想マイク形式を表示する。仮想マイクと内部PCMは32bit floatであり、物理入力24bitを表示する場合もある。音声の録音保存は行わない。

## ビルド

macOS 13以上、Xcode Command Line Tools、Rust 1.98.1（最低対応1.85.0）、Apple Silicon/IntelのRustターゲットを使用する。

```sh
bash scripts/test.sh
bash scripts/build.sh
```

通常版Universalアプリとインストーラを`dist/`へ生成する。ビルドはインストールや音声サービスの再起動を行わない。試験版は`bash scripts/package-rust-test-app.sh`で別UID・別設定先へ生成する。旧Go版の説明は[LEGACY_GO.md](LEGACY_GO.md)、ビルドは`scripts/build-legacy-go.sh`である。

## 検証範囲

E1x2での2時間通話でケロケロ音が再発しなかったことを利用者が確認した。実HALで44.1〜192 kHzの仮想転送と診断取得を確認した。RME、8時間の実経路、物理レート変更・切断復帰・全対応OS/Intel実行の網羅試験は未検証である。2時間の報告ではmono/stereoの条件は特定していない。正式版への切り替えは利用者の判断によるもので、これらの合格を意味しない。

配布物の署名はad-hocであり、Developer ID署名・公証は未対応である。詳細は[RUST_PROGRESS.md](RUST_PROGRESS.md)を参照する。GPL-3.0-only。依存ライセンスと対応ソースはアプリに同梱する。

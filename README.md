# ChannelBridge

多チャンネルのオーディオインターフェースから必要な入力を選び、2チャンネルの仮想マイク **ChannelBridge Mic** へ送るmacOSアプリである。Rustで実装したアプリと独自HALドライバを使用し、BlackHoleやLadioCastを必要としない。

## 導入

[GitHub Releasesの0.5.8](https://github.com/stfxv/ChannelBridge/releases/tag/v0.5.8)から`ChannelBridge-0.5.8-universal.pkg`を取得する。ローカルでビルドした場合は`dist/ChannelBridge-Rust.pkg`を使用する。インストーラを開いてアプリとドライバを導入し、Macを再起動する。アプリは`/Applications/ChannelBridge.app`に配置される。更新前のバックアップと復帰手順は[RUST_MIGRATION.md](RUST_MIGRATION.md)を参照する。

入力機器と入力チャンネルを選び、マイク使用を許可して送出を開始する。monoは選択した1入力を左右へ送り、stereoは選択した左右の入力をそれぞれ送る。mixは左右の入力の平均を両方の出力へ送る。通話アプリ側は **ChannelBridge Mic** を入力に選ぶ。

送出開始前から入力レベルを確認できる。画面には選択した入力のdBFS、物理入力形式、仮想マイク形式を表示する。仮想マイクと内部PCMは32bit floatであり、物理入力24bitを表示する場合もある。音声の録音保存は行わない。

「復帰時に自動送出」をONにすると、システムのスリープ復帰、または選択した入力機器の再接続後に送出を開始する。初期値はOFFで、設定は保存される。ONにする操作や入力機器の変更だけでは送出を始めない。起動時に入力機器が接続済みの場合も入力確認から始める。ミュートとチャンネル選択は維持する。停止操作は次の復帰まで有効で、同時に検出した自動復旧より手動の停止を優先する。

## ビルド

macOS 13以上、Xcode Command Line Tools、Rust 1.98.1（最低対応1.85.0）、Apple Silicon/IntelのRustターゲットを使用する。

```sh
bash scripts/test.sh
bash scripts/build.sh
```

通常版Universalアプリとインストーラを`dist/`へ生成する。ビルドはインストールや音声サービスの再起動を行わない。試験版は`bash scripts/package-rust-test-app.sh`で別UID・別設定先へ生成する。ビルドと対応ソースの検査は[RUST_DISTRIBUTION.md](RUST_DISTRIBUTION.md)、アプリの確認手順は[RUST_APP_TEST.md](RUST_APP_TEST.md)を参照する。

全体の構成は[ARCHITECTURE.md](ARCHITECTURE.md)、Rustの開発規則と自動検査は[RUST_STYLE.md](RUST_STYLE.md)に記載している。

## 検証範囲

0.5.8公開時に通常・productionの各131件のワークスペーステストとUniversal配布物の検査を実施した。音声処理のテストと測定条件は[RUST_AUDIO_TEST.md](RUST_AUDIO_TEST.md)、実HALの検証手順と診断プロトコルは[RUST_HAL_TEST.md](RUST_HAL_TEST.md)を参照する。FIFOの転送には処理予算を設けているが、1回の読み出しは残予算を超える場合があり、厳密な実時間期限を保証しない。

RME、8時間の実経路、物理レート変更・切断復帰・全対応OS/Intel実行の網羅試験は未検証である。0.5.8公開時に実機音声品質の追加確認は行っていない。

アプリとドライバの署名はad-hocで、インストーラは未署名である。Developer ID署名・公証は未対応である。GPL-3.0-only。依存ライセンスと対応ソースはアプリに同梱する。

# Rust音声処理の検証範囲

音声処理とネイティブアダプタのテストは、物理入力や仮想マイクを開かずに実行できる。実際の機器を使う手順は[RUST_HAL_TEST.md](RUST_HAL_TEST.md)を参照する。

## 検証コマンド

リポジトリのルートで実行する。以下のコマンドはHALデバイスを開かない。

```sh
source scripts/rust-env.sh
cargo test --locked -p channelbridge-audio
bash scripts/test-rust.sh
```

`channelbridge-audio` のテストはDSPとキューの検査を行う。`scripts/test-rust.sh` はワークスペースのテストに加え、確保検査、並行モデル、releaseでの応答格子、ABI比較、CFPlugInの模擬HALを実行する。実際にロードされたドライバや物理入力の試験は含まない。

## 実装

- 自前の窓付きsinc補間。Kaiser β=12、基準の片幅96、係数表の密度512。表の線形補間と左右共通の重みを使い、DC利得を正規化する。係数表・作業履歴・FIFOは開始前に確保する。
- ダウンサンプリング時は最大消費比に応じて帯域を制限する。細かなクロック補正のたびに係数を作り直さない。
- サンプル位置は整数のカーソルと0〜1の小数位相に分け、大きな絶対位置の浮動小数加算を避ける。
- 機器のサンプル時刻と同じサンプルのホスト時刻から速度を推定する。コールバック到着時刻を機器の時計と混同しない。
- FIFOと補間履歴を合算した充足量をゆっくり補正する。最大補正は±2000 ppm、変化速度は200 ppm/秒。後段のサンプル位置を飛ばさない。
- 出力DSPへFIFO・補間・補正制御を接続。開始時は安全量を待つ。不足時は有効な出力の境界を64フレームで処理し、欠けた範囲だけ無音にする。旧履歴は再利用せず、CoreAudioの経路は再作成しない。

方式の参照: Julius O. Smith著 [Windowed Sinc Interpolation](https://dsprelated.com/freebooks/pasp/Windowed_Sinc_Interpolation.html)。ChannelBridgeのコード・係数は新規実装で、既存リサンプラーの実装を取り込んでいない。

## 個別の検査

音声処理中のメモリ確保と解放は、DSPとネイティブアダプタで個別に検査する。リングとメーターの並行動作はLoomで検査する。

```sh
cargo run --locked -p channelbridge-audio --example realtime_allocations
cargo run --locked -p channelbridge-macos --example native_realtime_allocations
cargo test --locked -p channelbridge-audio --features model-check model_tests
```

補間フィルターの利得、位相成分、帯域外の減衰を調べるには、release構成で応答格子のテストを実行する。出力DSPの処理時間は `render_timing` で測定する。測定区間にはFIFOの読み出しと補間を含む。物理入力から通話相手までの遅延は実機で測定する。

```sh
cargo test --locked --release -p channelbridge-audio response_grid -- --ignored --nocapture
cargo run --locked --release -p channelbridge-audio --example render_timing
```

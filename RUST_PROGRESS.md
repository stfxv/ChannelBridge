# Rust再実装の進捗と検証範囲

2026-10-01更新。全体の到達点は `RUST_REWRITE_PLAN.md` に定義した、BlackHoleを含むアプリ・音声経路・仮想ドライバのRustでの再実装。

## 現在の実装

- Rustワークスペースを作成。音声アルゴリズム、設定、アプリ / ドライバの契約、HAL ABI、ドライバ中核を分離した。
- Rust 1.98.1を固定。最低対応は1.85。ローカルのRust環境は無視対象の `work/rust-bootstrap/` に導入し、通常のシェル設定は変更していない。
- 既存JSONの設定読み込み、選択チャンネルの保持、壊れた設定のエラー、検証後の原子的保存、保存失敗時の一時ファイル削除。
- ホスト時刻から直接算出する整数ベースの仮想時計。長いポーリング間隔でも一回で現在の周期へ追い付き、時刻計算に呼び出し回数を使わない。
- 一つのフレーム位置正規化。負 / NaN / Infinity / 整数として正確に表せない時刻を拒否する。
- 世代と絶対フレーム番号で有効範囲を判断するリング。音声・タグ・公開状態を全て原子化し、読み手は共有データを消去しない。書き手同士の競合は有限回の試行で処理する。
- 1フレーム不足でも有効な255フレームを保持。小数の時刻差だけで同じ整数位置の音を消さない。世代変更・リング周回後に古い音を読み出さない。
- ドライバ中核の欠落 / 拒否 / 不正時刻カウンタ。
- Apple SDKに対応するHAL関数表・IO時刻・ストリーム形式のRust宣言。プラグインABIにはアプリ側HAL APIを含めない。
- Rustの最低版 / 固定版のCI、Apple Silicon / Intelのコンパイル、SDKとのABI比較を追加。
- HALファクトリ、IUnknown、プロパティ、クライアント管理、開始停止、タイムスタンプ、実際のReadInput / WriteMix関数表を接続。
- 専用ワーカーからレート変更を要求し、HALがI/Oを停止した後だけレート・時計・音声世代を更新。診断プロパティとHALへのカスタム型定義を追加。
- 現行版と異なるUID / バンドルIDのUniversal試験ドライバを生成するスクリプトを追加。ビルドはインストールや音声サービスの再起動を行わない。
- ドライバにリンクしないアプリ側HALクライアントcrateを追加。実デバイスのUID・名前・レート・入力/出力ch数を読み取る。
- 試験UIDだけを対象に、合成音の時刻・左右・欠落を検査するIOProcツールを追加。物理マイクは開始しない。
- アプリ側SPSC FIFOを追加。端点を複製できない型で書き手/読み手を一つずつに限定し、音声転送にロック・確保・待機を使わない。
- 入力の選択・モノラル/ステレオ/平均ミックスと、開始前も更新する入力メーターの処理を追加。送出の開始/停止と入力時刻の断絶には世代番号を付け、FIFOと補間履歴に残った古い音声を廃棄する。CoreAudio形式の入力バッファをこの処理へ渡すアダプタを追加し、模擬バッファでメーターとFIFOまで確認。入力/出力のネイティブバッファアダプタをDSPへ接続。出力はミュート中も音声を消費し、64フレームで音量を切り替える。登録・開始・停止・解除の所有権管理を追加し、合成音試験も共通化。停止/解除失敗または実行中コールバックが残る場合はコンテキストを保持する。入力/出力の準備オブジェクトを追加し、共通FIFO・メーター・設定スナップショット・世代付きクロック推定値を接続。IOProc単位の未使用ストリーム無効化と、試験UID限定で両IOProcを開始/終了するTestSessionを追加。試験UIDへのレート変更要求と、名目/入出力形式の変更完了確認を追加。試験セッションは入力レート一致前に開始しない。制御ワーカーからの自動追従・復旧接続と実機転送は未完了。
- 専用試験ドライバのインストーラと導入/削除/確認手順を `RUST_HAL_TEST.md` に追加。
- 自前の帯域制限付き補間、機器時刻からの速度推定、FIFO量を使う滑らかな補正制御を追加。アプリ側の出力DSPへ接続し、不足と再開の境界処理も実装。条件と未検証範囲は `RUST_AUDIO_TEST.md`。

## ローカルで確認済み

`bash scripts/test-rust.sh`:

- 通常のRustテスト55件。FIFO、補間品質・位相、時計速度・補正制御、PCMパイプラインの到着ジッター、不足からの復旧を含む。
- Loomによる並行操作のモデル検査1件。公開途中の新しい音声と古いタグを混ぜて返さない条件を確認。検査はpreemption bound 2の有限モデルであり、全ての将来の並行処理を証明したものではない。
- Rustfmt / Clippy。テスト・exampleを含めて警告なし。
- CのSDK照会とRustの実際のレイアウトの一致: AudioTimeStamp 64、IOCycleInfo 224、ClientInfo 24、PropertyAddress 12、HostInterface 40、DriverInterface 184、StreamFormat 40、RangedFormat 56 bytes。
- IO時刻・クライアント情報・DoIOOperationのフィールド位置も比較した。
- AppleのCヘッダーを使う模擬HALからRust動的ライブラリをロードし、ファクトリ・関数表の実呼び出し、UID変換・サイズエラー・診断定義、2クライアント、部分欠落・リング周回、7回の非同期レート変更・世代更新を確認。C側はASan / UBSan付き。Rustライブラリ全体がsanitizer対象だったとは扱わない。
- 最低対応Rust 1.85.0でのテストと、x86_64-apple-darwin向けのクロスコンパイル。Intel実機での動作試験とは区別する。

時計の「8時間」テストは仮想時刻の算出試験であり、8時間の実通話や実機クロックの同期検証ではない。
新しい8時間試験も制御・キュー量のモデルで、PCMの連続処理ではない。PCM試験は6つの速度差それぞれ4秒分。FIFO→出力DSPの128回の処理ではallocator検査で確保・解放0を確認したが、HALコールバック全体の計測は今後必要。

## 2026-10-01の実Macでの確認

- Rustから現行のCoreAudioデバイスを列挙。E1x2の48 kHz、10入力/8出力と現行ChannelBridgeを確認。
- 利用者の承認後、専用インストーラを開き、macOSの導入完了表示・root所有の設置先・パッケージレシートを確認。
- `/Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driver` の署名検証と、CoreFoundationのCFPlugInによるファクトリ発見・生成を確認。その生成先へ模擬HAL試験を実行して合格。これはCoreAudioによるロードとは別の確認。
- 物理フォーマットとアプリ向けフォーマットを別々に照会。E1x2では48 kHz / 物理24bit / アプリ向けfloat32 / 10入力を確認。入力バッファの複数ストリーム・ゼロバッファを扱うRustアダプタを追加したが、実入力コールバックではまだ使用していない。
- 最新の合成音試験でも試験UIDは未ロードで、I/O登録前に停止。合成音の実HAL試験は「ドライバ未ロード」で開始前に停止した。Mac再起動後の確認を依頼中。音声サービスの強制終了・既定デバイス変更はしていない。

## 未完了

1. 実HALでの `.driver` ロード・転送、全プロパティとクライアント寿命の追加監査。模擬HAL試験だけで実HAL対応済みとしない。
2. アプリ側の物理入力 / 仮想出力への接続、時計速度の公開、FIFO・補間・補正制御の実HALでの検証。全レート品質・CPU・長時間PCM試験。
3. Rust制御ワーカー・復旧・AppKit画面・メーター・権限の全面移植。
4. プロセスをまたぐ診断プロパティ、インストーラ・設定移行・旧版への復帰。
5. Discord / E1x2 / RMEでの長時間・複数読者・レート変更・切断・スリープ復帰の実機試験。

現行のアプリ / BlackHole由来のドライバはそのまま稼働している。別名の試験ドライバは設置済みだが、CoreAudioでの列挙・転送は未確認で、通常版をRustへ切り替えていない。

## 制御処理の追加（2026-10-01）

制御スレッド向けのControllerを追加。利用者の送出意図を接続状態から分離し、レート変更待機・プレビュー・送出・復旧・終了を管理する。再試行間隔は1秒から最大30秒へ増やす。停止/解除失敗では新しいセッションを開始しない。設定保存が成功するまで設定と音声経路を変更しない。模擬バックエンドで待機中の停止、再接続後の停止/開始意図、保存失敗、後始末失敗を確認。CoreAudioバックエンドを接続。選択した入力/試験仮想UIDの消失、ID/レート/形式/チャンネルの変更、コールバック失敗や2秒の停止を監視する。上限32件の操作キューと専用制御ワーカー、画面向け状態スナップショットを追加。停止/終了は通常キューの空きに依存せず、UIからの終了要求は非同期。試験では満杯キューとスレッド内所有者の生成/破棄を確認。AppKit画面・メーター情報への接続と実機動作は未確認。起動途中の失敗では登録済みの両IOProcを明示的に後始末し、原エラーと各後始末エラーを保持する。後始末成功なら通常の再試行へ戻し、コールバック保持が必要な失敗だけBlockedにする。エラー分類の模擬試験は確認済みだが、実HALの各失敗段階への注入試験は未実施。

## 表示データの接続（2026-10-01）

制御スナップショットへ入力dBFS・3秒保持ピーク・物理入力のrate/bit/float形式・仮想マイク形式を接続。選択チャンネルだけを通常取得し、全入力一覧はoverview指定時のみ生成する。無音は0 dBFSと区別し、無効値は表示対象にしない。AppKitでの表示は未完了。実機ではCoreAudioプロセスが9月27日から稼働し、10月1日の試験ドライバ導入後はOS/音声サービスの再起動がないことを確認。直近2時間の指定ログに読み込みエラーは見つからなかったが、未列挙の原因を確定したものではない。

## 復旧・権限境界の追加検証（2026-10-01）

全体のRust検証スクリプトを再実行し、通常53テスト、Loomモデル、SDK ABI比較、模擬HAL、FIFO/DSPの確保検査が通過。追加の制御試験では、起動失敗の後始末が確認済みなら待機後に再試行し、コンテキスト保持が必要ならRefresh/Startでも新経路を作らないことを確認。権限取り消し時の終了、権限復帰前の停止意図、Quit後の再接続禁止も確認。実TCC権限画面と実HALの復旧を検証したものではない。

ネイティブ入力/出力アダプタの確保検査を追加。10ch模擬入力・128周期でプレビュー、ミュート、不足、再開を含めて確保/解放0を確認。Rust 1.85/1.98とIntel向け全ターゲットのコンパイルも確認。実HALのコールバック時間・確保の検査とは区別する。詳細はRUST_AUDIO_TEST.md。

## 入力機器一覧の公開（2026-10-01）

入力機器のUID/name/channel/rate/physical形式を制御ワーカーから1秒間隔で公開。マイク権限の許可を設定しなくても読み取りだけで一覧を取得する。形式の照会失敗は項目に残し、列挙失敗は前回一覧とエラーを維持する。自分の通常/試験仮想マイクは入力候補から除外する。実Macで `cargo run -p channelbridge-macos --example inventory_worker` を実行し、E1x2の10ch/48k/24bitほか4入力機器を取得し、音声開始なしで終了を確認。設定書き込み、レート変更、IOProc登録は行わない試験である。AppKitの機器選択への接続は未完了。

## マイク権限境界（2026-10-01）

objc2 0.6.4 / Foundation・AVFoundation 0.3.2 / block2 0.6.2を固定し、OSのマイク認可状態の照会と、メインスレッドの明示操作からの許可要求を追加。要求前に実行バンドルの非空NSMicrophoneUsageDescriptionを確認し、重複要求は保留する。完了ブロックは原子状態だけを更新し、UIを操作しない。NativeBackendは交渉/開始前と稼働中にもOS状態を確認する。未決定/拒否/制限/未知状態を許可と扱わない分類試験が通過。実Macで状態照会のみ実行しAuthorizedを取得したが、これは今回の実行元の認可であり、新Rustアプリの認可ではない。新アプリのバンドルと許可ダイアログでの拒否/取消/許可試験は未完了。

根拠: [Apple authorizationStatus](https://developer.apple.com/documentation/AVFoundation/AVCaptureDevice/authorizationStatus%28for%3A%29)、[Apple requestAccess](https://developer.apple.com/documentation/avfoundation/avcapturedevice/requestaccess%28for%3Acompletionhandler%3A%29)、固定したobjc2-av-foundationの生成バインディング。Rust 1.85の対象crate試験とIntel向け全ターゲットコンパイルを確認。ドライバはこのcrateに依存せず、AVFoundationをドライバへリンクしない構成を維持する。

2026-10-01の最新チェックポイントで権限バインディング込みの全体検証を再実行。通常54テスト、並行モデル、ABI比較、模擬HAL、二つの確保検査が通過。音声coreとネイティブ側でexample名が重複していたため、ネイティブ側をnative_realtime_allocationsへ改名し、手順も更新。再実行ログではこの出力ファイル衝突を含む警告なしを確認。実HALの読み込み/転送、UIと長時間通話の必須条件は未完了のままである。

## 終了診断の追加（2026-10-01）

セッション終了は両IOProcの後始末を必ず試み、出力/入力の各エラー、送出停止スナップショットの反映結果、コールバック用メモリ保持の有無を一つのShutdownErrorへまとめる。最初のエラーだけを返して次のエラーを隠す動作を除去。模擬結果で両方の失敗と片方だけの保持情報、停止反映失敗、成功を検証。実HALの同時失敗を再現したものではない。対象crateの最低RustテストとIntel向け全ターゲットコンパイルが通過。

## AppKit起動土台（2026-10-01）

crates/appを追加し、固定objc2-app-kit 0.3.2でメインスレッド専用ウィンドウ・ラベル・終了デリゲートを構築。`cargo run -p channelbridge-app -- --smoke` を実Macで実行し、ウィンドウと内容を構築して表示せずに終了することを確認。音声・許可要求は行わない。これは画面仕様の移植完了ではない。入力選択・メーター・送出/ミュート・メニュー/Dockアイコン・ワーカー接続と目視確認は未完了。実HALの必須ゲートも未達であり、通常版の切替はしない。

## AppKitの機器表示（2026-10-01）

Screenへ読み取り専用の機器ポップアップ、物理rate/bit/入力数のラベル、状態ラベルを追加。制御ワーカーの初回一覧を表示へ渡す。非表示のsmoke実行で、指定UIDを選択し、不在UIDを未接続項目のまま表示して設定を改変しないことを確認。Rust 1.85の実Mac実行とIntel向け全ターゲットコンパイルを確認。ポップアップ操作・設定読み込み/保存・タイマーによる継続更新・メーター/送出操作・アプリ終了時の非同期ワーカー終了は未接続。現在は初回の読み取り専用スナップショットを使う表示土台であり、既存アプリの置換や画面の目視確認ではない。

## AppKitの継続更新と終了待機（2026-10-01）

初回一覧を待つ起動時のメインスレッド停止を除去し、メインRunLoopのcommon modesに50msタイマーを登録。制御スナップショットを継続表示する。終了デリゲートはワーカーへ非同期終了を要求し、終了済みスナップショットを確認してからAppKitへ終了許可を返す。実Macの非表示smokeではRunLoopを動かして機器一覧の更新と明示的なワーカー終了要求後の完了・joinを確認。通常のウィンドウ閉じる操作やAppKitの終了要求自体を試したものではない。Rust 1.85/1.98のsmoke、Intel向け全ターゲットコンパイル、workspace Clippyと整形が通過。操作可能な機器選択、権限要求、設定移行、音量表示、送出操作、アイコン、実HAL転送と長時間通話は引き続き未完了である。

## 試験アプリの設定読み込み（2026-10-01）

Rust AppKit起動時に試験版専用のApplication Support設定を読み込み、不在の場合だけ既存ChannelBridge設定を読み取り専用で取り込む。保存先はChannelBridge Rust Testへ分離し、起動時にはファイルを作成・書き換えない。壊れたJSONや不正な0チャンネル、読み取り失敗は画面の設定エラーへ残し、自動的に別設定へフォールバックしない。非表示smokeは実ユーザー設定を読み込まない専用構成を維持。移行試験で9/16ch・ステレオ・ミュートの保持、移行元の未変更、壊れた移行先の未変更、試験版設定の優先を確認。core/appの14試験とClippy、非表示smoke、最低Rustでのapp試験が通過。Rust 1.85のIntelターゲットはこの環境に未導入のため、その組合せのチェックは未完了。設定変更のUI操作と実ユーザー設定の目視確認は次工程である。

## 機器選択と保存結果の反映（2026-10-01）

AppKit機器ポップアップを制御ワーカーのConfigureへ接続。設定処理完了カウンターを成功/失敗の両方でスナップショットへ公開し、画面は保存待ちを表示して次操作を保留する。保存完了までは以前の選択を維持し、保存失敗を表示。設定読み込みエラーがある場合は操作を無効にする。機器一覧/保存UIDが変わったときだけメニューを再構築し、メニュー表示中は項目の更新を保留。操作は表示していたUIDへ対応付け、一覧の新しい順番へ再解釈しない。情報用の先頭項目で保存済み機器を消去しない。

実Macの非表示smokeで実CoreAudio機器列挙と隔離した一時保存先を使用。保存先をディレクトリにして書き込みコミット失敗を発生させ、旧設定と選択の保持を確認。その後同じ操作で保存を成功させ、ワーカー設定・保存ファイル・表示選択の一致を確認。表示中メニューの更新保留も模擬スナップショットで確認。音声権限はワーカーへ許可せず、IOProcや音声は開始しない。Rust 1.85/1.98のsmoke、core/appの14試験、Clippy、Rust 1.98のIntel全ターゲットコンパイルが通過。実際のマウス選択・メニュー開閉イベントや見た目の確認、チャンネル/モード/メーター/送出操作の移植は未完了である。

## 入力チャンネルとモードの画面接続（2026-10-01）

AppKitへ左右チャンネルとモノラル/ステレオ/平均ミックスの選択を追加。画面部品はrouting_controlsモジュールへ分離。Configureの保存結果を待って反映する既存経路を共用し、操作直後はいったん保存済みの値へ戻す。表示時の機器UIDとチャンネル一覧を使って操作を解釈し、未接続項目をCh 1へ変換しない。モノラルでは右選択を無効化して保存した右値は保持。開いているメニューの項目再構築は保留する。

非表示smokeで保存済みCh 9/16を2ch機器へ切り替えた場合の未接続表示、16ch復帰時の復元、左だけ変更した場合の右値保持、平均ミックス選択、モノラルの右操作無効化、メニュー表示中の更新保留を確認。隔離した実保存先へステレオと左チャンネルのConfigure操作を送り、保存結果とスナップショットの一致を確認。権限は許可せず音声を開かない。最低Rustの実Mac smoke、Clippy、Intel全ターゲットコンパイル、整形が通過。実際のマウス操作・表示の目視確認、メーター/権限/送出/ミュート操作、実HALと長時間通話は未完了である。

## 入力レベルと仮想形式の表示（2026-10-01）

input_metersモジュールへ左右の入力dBFSと3秒保持ピーク、仮想マイクのrate/32bit float表示を追加。送出・ミュートの状態から独立した入力側メーターを使用。モノラルや同一左右チャンネルでは重複する右行を隠す。MeterReaderから250ms以内の更新有無をfreshとして公開し、初期状態・古いコールバック・取得できないチャンネルを無音と区別する。未更新は「入力レベル未取得」、更新された無音は−∞ dBFSとする。初期タイムスタンプを未取得の専用値にし、時刻0で取得した無音も区別できる。

audio/app/macosの43試験が通過。追加試験は未取得/新鮮な無音/期限超過/0 dBFSの区別を検証。最低Rustの非表示smokeでは実AppKitラベルの文字列、右行の非表示、ミュート中の入力表示、仮想48k/32bit表記を模擬AudioViewで確認。整形・Clippy・最低Rustコンパイル・Intelコンパイルも確認。実入力を開いた表示、マイク権限UI、送出操作、実HALと長時間通話は未完了である。

## 権限・送出・ミュート操作の接続（2026-10-01）

AppKitへマイク許可要求、送出開始/停止、ミュート操作を追加。許可要求は明示ボタンから既存MicrophonePermission境界へ渡し、起動時は要求しない。通常実行では1秒間隔でOS認可状態を確認し、変化をPermissionChangedとして制御ワーカーへ渡す。非表示smokeではこの認可通知を無効化し、実入力を開かない。拒否/制限/未決定/確認不可/許可済みをボタン表示で区別。開始は権限・選択の妥当性・保存完了を条件にし、停止は送出意図があれば設定保存中や再接続待ちにも可能。ミュートはConfigureの保存完了経路を共用。制御状態とエラーを状態ラベルへ反映する。

実Macの非表示smokeで隔離した保存先へミュート操作を送り、保存ファイル・スナップショット・ボタン表示の一致を確認。模擬スナップショットで再接続待ち/保存待ちの停止操作が有効、終了済みでは無効を確認。整形、workspace Clippy、最低Rustコンパイル、Intel全ターゲットコンパイルが通過。実NSButtonマウスイベント、TCCダイアログ/許可取消、新アプリバンドル、実送出/停止は未検証。仮想ドライバの実HAL認識と転送が完了したと扱わない。

## 独立したUniversal試験アプリ（2026-10-01）

build-rust-app.shと専用Info.plistを追加。macOS 13を最低バージョンとしてApple Silicon/Intelをビルドし、既存Cアイコン・マイク使用説明・別bundle ID・GPLと依存ライセンスを含むソースを同梱。依存ソースはCargo.lockに固定してrust-vendorへ同梱し、展開先でoffline/lockedのappコンパイルが通過。既存通常アプリは置換しない。両Mach-OのminOS 13.0と両アーキテクチャ、ad-hoc署名を確認。Universalアプリ内の実行ファイルからNSBundle ID/非空マイク説明を検証し、非表示UI smokeも通過。CIには実機器一覧を必要としない明示的build-onlyとmetadata検証を追加。詳細はRUST_APP_TEST.md。これはDeveloper ID署名/公証、macOS 13/Intel実行、Dock表示、通常UI/権限画面や実HALを検証したものではない。

## 通常画面の実操作確認（2026-10-01）

Universal試験アプリを通常のウィンドウで起動し、CUAによる実際のモードメニュー選択、Ch 9選択、ミュートを確認。試験版保存ファイルのステレオ/Ch 9/ミュートと表示が一致し、再起動後も復元した。既存通常版の保存内容はmono/Ch 1/muted=falseのままである。マイク許可は要求していない。通常画面の配置を画像で確認し、権限未許可のBlocked表示を「入力確認にはマイクの許可が必要です」へ修正。標準終了メニューとCommand-Qのキー対応を追加。ウィンドウの閉じる操作と終了メニューの実操作後、CUAの終了結果とプロセス不在を確認。Command-Qのキー入力自体、音声稼働中の終了、TCCダイアログは未検証。修正版bundleの非表示smoke、Clippy、最低Rustコンパイルも通過。試験アプリは終了済み。メニューバー常駐アイコン、実HAL、長時間通話は引き続き未完了。

## メニューバー常駐の接続（2026-10-01）

status_menuモジュールでNSStatusItemとマイクのtemplateシンボルを追加。停止/送出中/ミュートで画像を切り替え、状態・画面を開く・送出/停止・ミュート・終了のメニューを提供。画像が取得できない場合はCB文字へフォールバックする。操作は既存デリゲートと制御ワーカーを共用する。ウィンドウを閉じても常駐し、macOSのreopenイベントで既存ウィンドウを再表示する。最低macOS 13との互換性のため、14以降のactivateではなく既存activateIgnoringOtherAppsを局所的なdeprecated許可で使用（SDKヘッダーのavailabilityで確認）。通常smokeではメニューバー項目を作らず非表示確認を維持。

workspace Clippy、非表示smoke、最低RustとIntelコンパイル、Universal再ビルド/bundle metadataが通過。通常画面を再度起動し、閉じる操作後もアプリが残り、再表示されたことをCUAで確認。ただし画面取得自体がreopenを誘発するため、独立したDockクリック試験と扱わない。通常終了メニューからの終了を確認。SystemUIServerの画面取得はtimeoutとなり、ステータスアイコンの目視やそのメニュー実クリックは未検証。試験アプリは終了済み。TCC、実HAL転送、長時間通話の必須ゲートは未達のままである。

## 全入力レベル一覧（2026-10-01）

overviewモジュールへスクロール可能な全入力一覧を追加。メイン画面のボタンで開き、表示中だけCommand::Overview(true)を送り、閉じるとfalseへ戻す。送信失敗は次更新で再試行する。UIは512行を上限として機器の入力数に合わせ、未取得/無音/dBFS/保持ピークを既存表示関数で統一。非表示期間は行の更新を停止する。通常画面の実ボタンで一覧を開き、E1x2のCh 1〜10が表示されること、閉じて通常画面へ戻ることをCUAの画像/AXで確認。権限未許可なので値は未取得であり、実音声の全チャンネル測定ではない。非表示smokeでは行の生成/機器不在時の除去/復帰を確認。Clippy、最低Rust/Intelコンパイル、Universal再ビルドとbundle metadataが通過。試験アプリは終了済み。

同日の実HAL loopback再確認は引き続き「Rust test driver is not loaded」で音声開始前に終了。ドライバ認識・転送・長時間通話ゲートは未達であり、既存版への切替や安定性の達成を意味しない。

## 全体検証とCIの外部停止要因（2026-10-01）

現在のAppKit/overviewまで含む全体検証を再実行し、通常58テスト、Loomモデル、Apple SDK ABI比較、模擬HALのfactory/clients/audio/rate/epoch、二つの音声処理確保検査（確保/解放0）が通過。実HALや8時間の実音声を確認したものではない。メニューバー用objc2-core-foundation featureの依存辺をCargo.lockへ反映し、test-rust.shのCargo解決操作へ--lockedを追加。依存関係が検証中に暗黙変更されないようにする。

[GitHub run 36806987421](https://github.com/stfxv/ChannelBridge/actions/runs/36806987421)は両Rustジョブともstepsなしで失敗。Checks APIのannotationは「recent account payments have failed or your spending limit needs to be increased」と報告。従ってCI上でコードが検証されたとは扱わない。課金/利用上限の変更は実施していない。実機ゲートには試験ドライバの認識が必要であり、音声サービス再起動は通話等を中断し得るため、既存のインストール許可とは分けて確認を依頼した。

## 制御ワーカー異常終了の通知（2026-10-01）

WorkerExitガードをバックエンド生成前に登録し、制御スレッドがpanicでunwindした場合もfinished/Blocked/送出意図解除/音声表示解除を公開する。これによりAppKitがfinishedを待ち続ける状態を避ける。ガードはバックエンド等のスタック所有者が破棄された後に通知し、joinのpanic結果は成功へ変換しない。最後の公開設定を保持して新たなStartを拒否する。起動factoryのpanicと、模擬バックエンドの保存中panicの二試験で、終了通知・バックエンドDrop・旧スナップショット・join失敗を確認。最低Rustのcore 15試験、workspace locked Clippy、非表示AppKit smokeが通過。これは実HAL稼働中のpanic後始末、二重panicやプロセス強制終了の試験ではない。再起動に関する回答は未受領であり、音声サービスは変更していない。

## AppKit構成整理とメーター履歴モデル（2026-10-01）

前チェックポイントで926行程度のnative.rsを、起動/終了/セレクタを持つnative.rs、表示と保存反映を持つnative/screen.rs、隔離した保存試験のnative/smoke.rsへ分離した。内部の公開範囲は親モジュールに限定し、最低Rustの非表示smoke、Clippy、Intelコンパイルで既存の接続を維持することを確認。

メーター履歴のSlot::publish/readを独立した実装単位へ分け、Loom原子変数で上書きと読み取りをモデル化。履歴スロット周回中に新ピークと旧時刻を混在させない試験が最低/通常Rustで通過。全体スクリプトはmodel_testsフィルターへ変更し、リングとメーター双方を実行する。通常メーター二試験とネイティブ音声処理の確保/解放0検査も通過。実HALや実通話の合格と区別する。音声サービス再起動は許可済みだが管理者認証待ちであり、まだ実行していない。

## 音声サービス再起動のSIP拒否（2026-10-01）

利用者が許可済み再起動コマンドを管理者権限で実行したが、launchctlは150（System Integrity Protection engaged）で拒否した。再確認ではcoreaudiod PID 723/audiomxd PID 858の開始日時は9月27日のままで、試験UIDも未認識。従って音声サービスの再起動や実HAL検証が完了したとは扱わない。この環境へ案内したkickstart手順は使用できないと判明した。SIPや他の保護設定を変更せず、RUST_HAL_TEST.mdの通常Mac再起動による認識確認へ戻す。通常再起動後に認識する保証や、未認識の原因確定とは区別する。

## Mac再起動後の実HAL転送（2026-10-01）

利用者の再起動完了後、coreaudiod PID 776の開始日時が10月1日12:00:42、audiomxd PID 914が12:00:54へ更新されたことを確認。実HALの列挙にChannelBridge_Rust_Test_UIDが48 kHz・入力2ch/出力2chで現れた。3秒の合成音loopbackはcallbacks=750、compared_frames=143488、silent_frames=512（初期未書込）、missing_after_startup=0、mismatched_frames=0、invalid_callbacks=0で終了コード0。これは実HALのロードと短時間転送・停止の合格であり、物理入力の時計差補正、Discord、長時間安定性の合格ではない。導入直後の未認識が通常再起動で解消した観測であり、他環境でのロード保証や根本原因の確定ではない。

最新ソースからUniversal試験アプリを再ビルドし、署名/bundle metadataと非表示smokeが通過。通常画面でE1x2の48 kHz・24bit・10入力表示を確認。別アプリのマイク許可は未決定で、実入力開始前に利用者へ許可を確認中。既存通常版の置換や既定入出力変更は行っていない。

最新全体スクリプトは通常60試験、Loom 2試験、SDK ABI比較、模擬HAL、確保/解放0検査が通過。48 kHz実HAL loopbackをさらに3回開始/停止し、比較143680/143872/143680フレームで欠落・不一致・不正callbackはいずれも0。

rate_loopback例を追加し、停止中の7段階レート変更後の転送と元レートへの復元を検査する。最初の44.1 kHz段階では開始後512フレーム欠落を検出し不合格。その後の再試行は追従timeout、さらにNOT_READY（1852990585）となった。一方CUAが利用者による画面変更を検出し、再取得時にはマイク許可済み・Ch 1/2・送出中と実入力dBFS表示を確認。coreaudiodログにも試験アプリの実IO稼働が記録されていた。従って合成音と別送出の競合を除外できず、当該欠落を単独ドライバ不具合として扱わない。Command-Q後のアプリ終了を確認したが、その後もNOT_READYが再現し、レート変更の実機ゲートは未達。48 kHzへの復元後の列挙では仮想・物理形式とも48 kHzだった。最初の欠落sampleを報告する診断値をloopbackへ追加し、関連単体試験とClippyが通過。元ドライバの時計差・実機レート変更の切り分けは継続する。

## レート変更拒否後の再要求と実入力プレビュー（2026-10-01）

旧実装ではPerformDeviceConfigurationChangeがI/O稼働等を理由に拒否するとpending_rateが残る。ホストがAbortを呼ばなければ以後の変更がNOT_READYとなる経路である。今回の実機で当該callbackが拒否された証拠は取得できておらず、症状の原因確定とはしない。PendingChangeガードにより、該当する要求のPerform終了時に成功/拒否とも待ち状態を解除し、異なる要求は解除しない。模擬HALで稼働中拒否→停止→旧要求の再実行拒否→新規非同期要求受理→変更成功を確認。workspace通常60試験、Loom 2試験、模擬HAL/ABI/確保検査、最低Rust全ターゲットcheckが通過。

利用者操作で許可された試験アプリを再起動し、送出停止・ミュート中もE1x2のCh 1/2の実レベルが更新されること、全入力一覧でCh 3〜10が−∞として無音と区別されることをCUAで確認。これは物理入力プレビューの確認であり、仮想マイクへ同じ波形が届くことや長時間安定性の証明ではない。Command-Qで試験アプリ終了を確認。その後の独立48 kHz合成音はcallbacks=752、比較143872、初期無音512、開始後欠落/不一致/不正callback=0。

修正版は試験ドライバ0.5.1/build 2としてUniversalビルド・CFPlugIn経由模擬HAL・署名確認が通過。パッケージを展開し、版0.5.1、ペイロードの実行ファイルとビルド品の一致、展開後署名を確認した。pkgbuildのwrite: Permission denied警告4件は再現しており、成功終了とペイロード確認だけで警告原因を解決済みと扱わない。パッケージ版はInfo.plistから読み、二重管理をなくした。更新インストーラを開いてインストールを選択したが、管理者認証完了や実際の新版ロードはまだ確認していない。認証は利用者へ引き継ぎ、実機レート変更ゲートは未達のままである。

その後、インストーラの完了表示を確認。pkgutil receiptと配置先Info.plistは0.5.1で、配置済み実行ファイルと検証済みビルドが一致し、配置先署名検証も通過した。利用者のシステム画面での認証により導入が完了した観測であり、こちらが認証情報を取得したものではない。ドライバホストPID 800、coreaudiod PID 776、audiomxd PID 914はいずれも12時台の起動日時のまま。独立rate_loopbackは依然NOT_READYで開始前終了。従って配置の更新と新版ロード/実機動作を区別し、新版の再試験に向けて利用者へ通常Mac再起動を依頼した。試験アプリは終了済みである。

## 実入力から仮想マイクへの読み取り検査（2026-10-01）

再起動待ちの間にmonitorモジュール/例を追加。試験UIDだけを読み、そのIOProcの出力ストリームを開始前に無効化する。既定機器/レート変更、合成音の書き込み、許可要求、音声保存は行わない。左右別の非ゼロフレーム数と絶対ピーク、非有限サンプル、不正callbackだけを集計する。単体試験で左右・符号付きゼロ・NaN/Inf・短いゼロバッファヘッダーを区別。最低RustとIntel全ターゲットcheck、workspace通常61試験、Loom 2試験、既存確保/ABI/模擬HAL検証が通過。新monitorのcallbackは既存確保検査の対象ではない。

E1x2入力48 kHz/24bit、試験アプリのステレオ設定で、実HALの読み手を3秒ずつ開始して次を確認した。

| 状態 | フレーム数 | 左/右の非ゼロフレーム数 | 左/右ピーク |
| --- | ---: | --- | --- |
| アプリ終了中 | 144384 | 0 / 0 | 0 / 0 |
| 入力プレビュー・送出停止 | 144384 | 0 / 0 | 0 / 0 |
| Ch 1/2送出・ミュート解除 | 144384 | 144384 / 144384 | .019943045 / .019943045 |
| 送出中ミュート | 144384 | 0 / 0 | 0 / 0 |
| 左Ch 3・右Ch 2送出、読み手A | 144576 | 0 / 144576 | 0 / .010867656 |
| 同じ送出を同時に読む読み手B | 144384 | 0 / 144384 | 0 / .010867656 |
| ミュート解除のまま送出停止 | 144384 | 0 / 0 | 0 / 0 |

全検査で不正callback/非有限サンプルは0。Ch 3の無音とCh 2の信号は入力メーターでも確認し、左右の対応と二読み手の短時間利用を検証した。無音化/信号の到達は確認できたが、波形一致、位相、遅延、長時間ドロップアウト、Discordでの品質、全モード、機器切断や睡眠復帰の合格とは扱わない。ドライバホストは起動日時が更新前のままであり、これらを修正版0.5.1のロード証明ともしない。試験前のCh 1/2・stereo・muted=trueへ設定を戻し、送出停止後Command-Qで試験アプリを終了した。試験設定ファイルの復元と既存通常版のmono/muted=falseを確認。

## Rust専用の対応ソース配布（2026-10-01）

従来のRust試験アプリのソースarchiveには旧Go/BlackHole実装と旧ビルド手順も含まれていた。package-rust-source.shへ同梱処理を分離し、Rust crates/manifest/lock、Rustビルド・検証スクリプト、専用plist、ChannelBridgeアイコン、依存元ソースとライセンスだけを含める構成へ変更。旧Goのcmd/internal/go.mod、旧driver/vendor/BlackHoleと旧ビルド・音声試験のC/Objective-Cは除外する。Rust用C ABI/模擬HAL検証プログラムは対応ソースへ残すが、製品音声経路へリンクしない。THIRD_PARTY_RUST.mdとRUST_DISTRIBUTION.mdを追加し、試験アプリのnoticeも専用内容へ変更。旧版の原ソースとライセンスはリポジトリに保持する。

Universalアプリの署名、bundle ID/マイク説明検証、非表示UI smokeが通過。verify-rust-source.shで同梱archiveの許容root/必須ファイルを確認し、展開先を作業ディレクトリとして空のCARGO_HOME・offline/lockedで全ワークスペース全ターゲットcheckが通過した。manifest-pathだけを元repoから指定する確認とは区別し、展開先の.cargo設定を実際に使用した。cargo vendorにもrespect-source-configを付け、展開先の依存置換設定を尊重する。検証をCIへ追加したが、GitHubの課金停止が解消したとは確認しておらずCI合格と扱わない。新helper初回のシェル構文ミスは検出・修正し、最終版の構文確認/アプリ再ビルド/空キャッシュ再検証が通過。再起動前の音声プロセスが続いており、実機レート追従ゲートは未達である。

## 出力DSPの処理時間測定（2026-10-01）

再起動前の音声プロセスが引き続き稼働しており、実機レート変更を合格と扱わず、独立して音声処理の負荷を測定。render_timing例でOutputProcessor::renderをreleaseビルドし、16レート×512フレームと優先4レート×64/192/4096フレームの計28条件を実行。各64回のウォームアップと256回の測定で出力不足/primingなしを確認。M2 Max・Rust 1.98.1で48 kHz/512のp99は0.21050 ms（期限の1.97%）、192 kHz/512は0.21029 ms（7.89%）、768 kHz/512は0.22575 ms（33.86%）。生データRUST_RENDER_TIMING.tsvと測定範囲をRUST_AUDIO_TEST.mdへ記録した。

この値は同一名目レートでFIFO読み取り・補間を含む出力DSPだけの経過時間である。OSのプリエンプションは含むが、物理入力、HAL、ドライバ、画面、構築、異レート変換や復旧は計測していない。全CPU使用率・追加音声遅延・実機での締切保証へ一般化しない。最低Rust/IntelコンパイルとClippyが通過。初回の測定例ではFIFO pushの戻り型を誤りコンパイルが失敗したが、修正後の全28条件が成功した。生データもRustソースarchiveへ含める構成へ更新した。

更新後Universal試験アプリのビルド/署名/bundle metadataと、空Cargoキャッシュ・offline/lockedの同梱ソース全ターゲットcheckが通過。archive内の測定例と生データが作業ツリーと一致することも確認した。通常画面や音声経路は起動していない。

## フィルター応答の格子検証（2026-10-01）

単純な数点の応答試験に加え、最大step 0.501/1.002/2.004/4.008/16.032/96.192、補間位置の小数位相16点、帯域内32点、downsample時の帯域外33点を実際のSincKernel::interpolateで検査するignore付きrelease試験を追加。合計5712応答で単位cos/sin入力から利得と理想位相成分との差を測定した。Rust 1.98.1の最大帯域内利得誤差0.00003232 dB、最大位相成分誤差0.00000359、測定帯域外最大−117.0785 dB。既定の0.1 dB/−90 dB目標と位相成分誤差0.001以内を格子内で満たした。生データをRUST_RESPONSE_GRID.tsv、周波数/位相/除外帯域と制限をRUST_AUDIO_TEST.mdへ記録。

最低Rust 1.85.0でも同じrelease試験が通過。test-rust.shへ明示release実行を追加し、全体スクリプト（通常61試験、Loom 2、格子1、模擬HAL/ABI/確保検査）が通過した。通常の高速試験では格子は1件ignoreされ、別途実行される。これは固定フィルターの離散応答格子であり、全周波数/全補間位相、ASRC時間変動、群遅延/総合ノイズ、アップサンプルimage、全レート組合せ、実HALやDiscord長時間の合格とは扱わない。製品の係数や音声形式は変更していない。再起動前のドライバホストが継続しており、実機レート追従ゲートは未達である。

## アプリ・ドライバ統合試験インストーラ（2026-10-01）

package-rust-test-app.shで一つのpkgにUniversal試験アプリと試験ドライバを同梱する構成を追加。アプリ0.5.1/build 2とドライバ0.5.1/build 2の版一致を要求し、別のreceipt ID local.channelbridge.rust.test.pkgで生成する。導入先は/Applications/ChannelBridge Rust Test.appと/Library/Audio/Plug-Ins/HAL/ChannelBridgeRustTest.driverだけ。component plistで再配置を禁止し、厳密ID/版検査/upgradeを設定。通常版の名前/UID/配置先へは書かない。ビルドだけを実行し、統合pkgの実インストールはしていない。

両アーキテクチャのビルド、ドライバCFPlugInの模擬HAL、署名、bundle metadata、空キャッシュの対応ソースoffline/locked check、pkg展開後の両署名/両実行ファイルとソースarchiveのバイト一致が通過。PackageInfoに二bundleの専用ID、0.5.1/build 2、postinstall-action=none、relocatable=falseを確認。新bundleの非表示UI smokeも通過。12 MB程度のChannelBridge-Rust-Test.pkgを生成した。CIも一つの統合ビルド手順へまとめたが、CIの課金停止解除や実行合格は確認していない。

pkgbuildのwrite: Permission denied警告4件は継続しており、ペイロード確認の成功と警告原因の解決は区別する。実際の統合導入/更新/削除/復帰、ロードし直したドライバでのレート追従、Developer ID署名/公証、長時間通話は未完了。生成物は試験用途であり通常版への切替とはしない。

## ドライバ診断プロトコルの読み取り（2026-10-01）

既存cbst診断プロパティを読む制御側diagnosticsモジュールとprobe --diagnosticsを追加。試験UIDだけを読み、音声開始、既定機器変更、カウンタ消去は行わない。contractで48-byte native-endian protocol v1を明示的に解読し、サイズ・未対応版・予約値の不一致を拒否。非整列データと2^54を超える整数カウント、47/49 bytes、protocol 2、非ゼロreservedを単体試験で確認した。独立に採取された累積カウントを一括同時刻のepoch snapshotと扱わない。

最低Rustのcontract試験、Intel全ターゲットcheck、workspace Clippy、通常62試験/Loom 2/応答格子1/模擬HAL/ABI/確保検証を含む全体スクリプトが通過。Apple C ABIの模擬HALにもcbst読み取りを追加し、255書込/256読出に対するgenerationとwritten=255、missing=1、rejected=0、invalid=0およびreserved=0を確認した。ドライバ側の既存wire形式や製品音声処理は変更していない。

実HALへのreadはUnknownProperty（2003332927）で終了した。音声ホスト/ドライバホストの起動日時は更新前のままであり、古いロード状態の影響が考えられるが原因の確定とはしない。値を取得できないことをカウント0/無欠落へ変換していない。新版ロード後に実カウンタの読み出しと差分・世代の評価が必要である。累積missingは起動区間や書き手不在も含み、値の増加だけで今回の通話障害の原因とは判断しない。

## 起動前のドライバ互換性確認（2026-10-01）

計画の「互換性のないドライバには送出しない」に合わせ、NativeBackendのレート交渉前と音声IOProc登録前に診断プロトコルの読み取りを要求した。取得失敗、未対応protocol/size/reserved、未初期化generation=0ではエラーを返し、音声セッションを開始しない。無音や書き手不在を互換性不良と混同しないよう、累積欠落値には開始可否の閾値を設けない。未初期化を拒否し、初期化済み・書込0・欠落最大値でも受理する境界を回帰試験で確認した。実行中の連続互換性監視を追加したものではない。

macosの17試験、workspace Clippy、書式検査が通過。先行する同改修の全体検証とUniversalアプリビルドも通過したが、実画面は新ビルドでマイク許可未確認となり、許可段階で停止したため互換性エラー表示の実機合格とは扱わない。署名変更と権限状態の関連は推定であり、署名要件を弱める対処はしていない。ユーザーの再起動完了報告後も、確認できるMacの起動時刻は11:59:17、音声ホスト12:00:42、ドライバ設置15:34:35であった。実診断読み取りはUnknownPropertyのままであり、新版ロード・実レート追従・長時間通話の検証は未完了である。

## 出力の初期バッファ待ちと音声不足を区別する診断（2026-10-01）

OutputStatsにpriming_callbacksとdropped_lookaheadを追加。既存missingは実際に処理を開始した後の不足フレームであり、初期/復旧バッファ待ちは含まなかった。今回それらを別の累積値として公開し、復旧時に破棄した補間先読みフレームも記録する。音声callbackは既存の所有状態から報告を受け、固定数のatomic更新だけを追加する。制御側はatomicsのみ読み、個別値の取得を同時刻の整合snapshotとは扱わない。停止中はDSPを実行しないためprimingとして数えず、ミュート中は従来どおり消費・診断を続ける。音声の処理/バッファ/復旧方針は変更しない。

実バッファを用いる回帰試験で、空の初期入力では無音・priming=1・missing=0、信号を供給してから供給停止した場合はmissing/先読み破棄/再primingが増え、callback拒否やpanicはないことを確認。workspace Clippy、最低Rustの全macosターゲットcheck、全体test-rust.sh（Loom/応答格子/C ABI/模擬HAL/確保検証を含む）が通過。最初の直接script実行は実行権限なしで失敗し、bashによる本来の実行方式で完了した。これは診断採取の基盤であり、画面の「診断を保存」、FIFO範囲/補正量/入力時計、実機長時間試験は未完了である。

## 入力側の時計と経路異常の診断（2026-10-01）

CaptureStatsにDSPが検出したinvalid_clock、discontinuities、unavailable_channelの累積callback数を追加。これまでrejectedへまとめられた原因の一部を区別できる。discontinuitiesは時計の連続性喪失だけでなくFIFO overflow等の世代破棄も含み、時計だけのカウンタとは扱わない。DSPに届く前の不正native buffer/now descriptorや競合はrejectedのみに計上し、新カウンタへ誤分類しない。複数理由が同一callbackで成立するため足し合わせてcallback総数にはできない。診断追加は固定数のatomic更新であり、音声の経路/時計復旧方針を変更しない。

既存のnative buffer→DSP→FIFO試験へ、保存済みだが存在しないCh 3の経路と、input timestamp欠落による時計喪失を追加。前者はunavailable_channel=1/discontinuities=0、後者はinvalid_clock=1/discontinuities=1、panicなしを確認した。画面/保存ファイルでの公開、実機原因の確定、全診断要件の完成とは扱わない。

## 診断データの制御スレッド公開（2026-10-01）

coreの所有データAudioDiagnosticsへ入出力callback統計をまとめ、Backend→Controller→Worker Snapshotの経路を追加。NativeBackendはセッションのatomic統計のみを読み、HALアクセスや音声処理状態への参照、録音を行わない。データはJSON serialize可能な整数/booleanだけで、音声波形、機器UID、ユーザーパスは含まない。各値は独立読出しであり同時刻の整合epochとは扱わない。セッションなし、工場失敗、正常終了、panic終了ではNoneとし、過去セッションの値を現セッションとして保持しない。

worker試験で制御スレッドから非ゼロ診断が届くこと、2^54+3がJSON整数として保持されること、権限失効と終了でNoneになることを確認。初回試験コードはjoin後に消費済みworkerを参照してコンパイル失敗し、共有snapshotを事前保持する形へ修正。最低Rust全workspace/全ターゲットcheck、Clippy、全体検証が通過し、追加の権限失効確認後core 15試験とClippyも通過。診断保存UI/ファイル、FIFO状態と補正量、driver値の統合、再接続前後の履歴は未実装であり、本変更を全診断要件の完了とは扱わない。

## サイズ制限付き診断ファイル出力（2026-10-01）

core::diagnostics::save_newを追加。所有済みworker snapshotからschema version 1のJSONを作り、状態/送出意図/チャンネルとモード/ミュート/セッション統計/制御エラーを出力する。機器UID、機器一覧、ユーザーパスそのもの、音声波形はレポートの専用フィールドへ含めない。ただしエラー本文はそのまま出力するため、由来によってパス等が含まれる可能性はあり、完全匿名化とは称さない。セッション未取得はnullとする。64 KiB制限はserialize中のWriterで強制し、巨大な全文を先に生成してから拒否する方式にはしない。

既存の親ディレクトリに0600/create_newの一時ファイルを作り、sync後に同じディレクトリのhard linkで完成ファイルを公開する。既存の保存先は置換せず、失敗時は所有する一時ファイルを除去する。音声callbackからは呼ばず、画面から明示保存するための基盤である。回帰試験で未取得null、UID除外、0600、既存ファイル不変、巨大エラー拒否と一時ファイル残留なしを確認。Clippyと最低Rust全workspace全ターゲットcheckが通過。UI操作との接続とdriver統計/形式/時計補正量の統合は未完了であり、現時点の製品画面から保存できるとは扱わない。

## 「診断を保存」の画面操作（2026-10-01）

試験アプリに診断保存ボタンと結果表示を追加。AppKitのNSSavePanelで保存先を選び、ボタンを押した時点のworker snapshotを別スレッドでsave_newへ渡す。保存待ちの重複操作を禁止し、完了/失敗を既存UI timerで受け取る。音声スレッド/制御ワーカーでファイルI/Oをしない。保存処理中の終了要求では音声停止を先に要求し、保存結果を受け取ってからアプリ終了へ応答する。同名ファイルは置換せず、新しい名前を求めるエラーを表示する。診断用に画面下部を40 px追加した。

Clippy、最低Rust app全ターゲットcheck、Universalビルド/署名/metadata/非表示AppKit smokeが通過。最初のsmokeは旧部品数15の期待値で失敗したため17へ更新し、保存ボタンのtitle/有効状態/位置も確認した。実GUIで保存panel、新規保存、結果「保存しました」、キャンセル後ボタン有効、Command-Q終了を確認。ローカルhomeに生成された243-byteのJSONがschema 1/Blocked/session_counters=null/0600であることを読出し、試験フォルダwork/test/rustへ移動した。Go To経由の試験フォルダ指定は画面上homeのままであり、フォルダ移動成功とは扱わない。clipboard操作もtimeoutしたが、保存操作自体は通常のpanel操作で完了した。マイク許可/送出は行わず、送出中保存や保存中終了競合の実機確認、driver値/形式/時計補正の統合は未完了である。

## 保存診断へのドライバ統計統合（2026-10-01）

Backend/Controllerにドライバ診断の読み取り口を追加し、制御ワーカーの機器一覧更新と同じ1秒周期で取得する。音声セッション/マイク許可の有無に依存せず試験UIDのread-onlyプロパティを読み、UI更新ごとにHALを呼び直さない。Worker SnapshotとJSONへ、取得時のworker経過msと、Available（generationと累積5指標のうちgeneration以外4値）/Unavailable（エラー本文）の明示状態を加えた。未取得はnullであり、失敗時に過去の成功値を残さない。終了/panicではドライバsnapshotもnullへ戻す。ドライバとアプリの統計は独立に取得され、同時刻の一括epochとは扱わない。

core 16試験、Clippy、最低Rust全workspace/全ターゲットcheck、全体スクリプトが通過。export回帰試験でUnavailableにwritten_framesが存在しないことを確認。実機のread-only inventory workerからsave_newまでの経路も実行し、391-byte/0600のファイルへobserved_worker_ms=148、Unavailable/CoreAudio status 2003332927、session_counters=nullが保存されたことを検証した。診断取得失敗をworkerの音声開始成功や無欠落へ変換していない。現ロードでの成功カウンタ取得、更新後アプリでのGUI保存、FIFO/補正量/形式/履歴の実装と長時間試験は未完了である。

## FIFO量と時計補正の診断（2026-10-01）

OutputCallbackのatomic統計へ、DSP生成フレーム数、FIFOの処理後現在量・処理前後で観測した累積最小/最大量、総入力バッファ目標量、最新の補正ppmを追加し、NativeBackend→worker→JSONまで接続した。目標はFIFOと補間historyを合わせた量であり、FIFO単独の目標ではない。生成フレーム数にはミュート中のDSP処理も含み、ユーザーへ聞こえたフレーム数とは扱わない。未観測の最小/最大/現在/補正はnullで、最初のrender後に値を公開する。停止中は処理せず最後の観測を保持する。量と補正は独立atomic取得であり同一時刻の完全なsnapshotではない。

最初の回帰試験で処理後FIFOだけの最大値が0となり失敗した。renderがFIFOから補間historyへ全入力を移すことを確認し、最小/最大へ処理前後の両観測を含めるよう修正。callback間の全瞬間ピークを保証する値ではなく、開始/復旧時の空状態も最小値へ含むため、最小0単独を障害と判断しない。空状態のnull→0/補正0、信号処理後の生成増加/FIFO最大非ゼロと不足復旧を検証した。Clippy、最低Rust全workspace全ターゲットcheckが通過。観測クロック速度、正確な形式/版、レート要求履歴、仮想出力レベルと実機長時間ゲートは未完了である。

## 観測入力時計と世代の保存診断（2026-10-01）

既存SourceClockの単一AtomicU64に格納されている世代と速度を、snapshotとして制御側へ公開。CaptureStats/AudioDiagnostics/JSONへcapture_clock_generationとobserved_source_speed_ppmを追加した。音声callbackに重複した時計用atomicを増やさず、出力DSPが利用する既存の量子化済み速度を使用する。速度差は名目入力レートに対する推定値であり、物理時計の直接測定や最終出力補正値と同一とは扱わない。保持精度は既存通り速度1e-8（ppmの刻み0.01）。世代と速度だけは同一atomic wordから取得し、他の累積値やdriverとの同時刻整合を主張しない。

未公開は世代/速度ともnull、時計を失った後は世代あり/速度nullとし、速度差0へ変換しない。SourceClock試験で初期None、正常公開と古い世代拒否、NaN/範囲外の速度Noneを確認。native buffer試験でも通常送出の世代/速度1.0、timestamp欠落後の世代進行/速度Noneを確認しmacos 18試験が通過。Clippyと最低Rust全workspace全ターゲットcheckが通過。実機長時間の推定精度、形式/版/履歴、仮想出力レベルは未検証・未実装の範囲が残る。

## 入力・仮想形式と入力レベルの保存診断（2026-10-01）

InputFormat/InputLevel/AudioViewをserialize可能にし、レポートにアクティブセッションのaudioを追加した。物理入力のrate/bits/channels/float、仮想rate/bits、選択チャンネルのdry dBFS/freshnessを保存する。無音のdBFSはnullでfresh=true、未取得はaudio=nullまたはfresh=falseで区別できる。機器一覧の最終読出しに含まれる選択機器の名目レート/物理形式/形式取得エラーもselected_input_inventoryへ保存し、アクティブセッションの形式証拠と混同しない。名前/UIDを専用出力フィールドに追加しない。

保存回帰試験で物理24bit signed整数/10ch/48kと仮想32bit/48kの区別、dry無音のfresh=true/null、機器名とUID除外を確認した。追加の別ファイルでセッションと機器不在の両フィールドnullも確認。最低Rust全workspace全ターゲットcheck、Clippy、全体検証が通過し、nullの追加ケース後も保存試験/Clippyが通過。アプリ/ドライバの版、レート要求履歴、仮想出力レベルと実機長時間ゲートは未完了であり、全診断要件の完成とは扱わない。

## 仮想出力の実レベルを保存診断へ追加（2026-10-01）

出力callbackでフェード/ミュート適用後の実HAL bufferの左右ピークを求め、一つのAtomicU64へ二つのf32 bitsをまとめて公開する。制御側でdBFSへ変換しoutput_peak_dbfsに保存。最初の未観測または非有限PCMを含むブロックはnull、観測済み左右無音は[null,null]（−∞ dBFS）で区別する。送出停止中の正常callbackとpanic時の無音化ではピーク0を公開する。不正buffer/競合で拒否されたcallbackは以前の値を保持するため、値は最新の受理ブロックの観測であり、常に最新の全callbackや現在の通話音量とは称さない。callback停止後の値の鮮度や実際の受信アプリの音量も保証しない。

既存native output試験で、未観測null、初期無音、左右0.5/0.25の独立ピーク、ミュート定常時/送出停止時0を確認。音声bufferやミュート処理の値は変更せず、診断の走査/固定atomic公開のみ追加した。Clippyと最低Rust全workspace全ターゲットcheckが通過。新版アプリでの保存操作/負荷の実機確認、版と復旧履歴、長時間通話は未完了である。

## 再接続・交渉の制限付き履歴（2026-10-01）

Controllerに交渉開始/交渉準備完了/セッション開始/再試行/終了失敗/権限失効/機器変更/経路失敗のイベント履歴を追加し、Worker SnapshotからJSONのcontrol_historyへ保存する。直近32件・理由本文最大256 UTF-8 bytesで古いイベントを除く。immutable Arcをイベント更新時だけ作り直し、画面snapshotではArcを共有する。音声callbackで履歴の確保/文字列処理をしない。復旧で現在エラーが消えても履歴は残り、正常終了時にも保持する。tick由来イベントの時刻はworker単調経過ms、UIコマンド由来は時刻なしとし不正確な時刻を付けない。

回帰試験で開始失敗→復旧後エラー消去/理由保持、以前のArc不変、40件後の最新32件、マルチバイト文字の255-byte境界を確認。worker試験で開始イベントがsnapshotへ届き、終了後にも権限失効履歴が残ることを確認。core 17試験、Clippy、最低Rust全workspace全ターゲットcheck、全体検証が通過し、追加worker確認後も対象試験/Clippyが通過。履歴は交渉段階の成功を表し、要求した正確なレート値/driver callbackの完了時刻をまだ収集しない。実機での障害再現、レート追従、版情報、長時間通話ゲートは未完了である。

## レート交渉履歴の要求値（2026-10-01）

RateFollowの保持するdesiredをread-only getterで公開し、Backendのrequested_rate経由でControlEventにrequested_rate_hzを追加した。交渉準備を開始した段階と、HAL設定要求からPendingを得た段階を分け、後者をNegotiationRequestedで記録する。Pending中の失敗/準備完了は消去前に要求値を記録する。要求を取得できない段階はnullとし、完了後の無関係なイベントへ以前のレートを流用しない。Readyは名目レートと全仮想/物理stream形式の一致の確認であり、最初の音声受信やdriver callbackの正確な完了時刻とは区別する。

入力の名目レートを読む処理をRateFollow::source_rateへ分離し、NativeBackendで設定要求の失敗理由にも要求Hzを含める。物理機器のレートは変更せず、読出し自体に失敗した場合は値を捏造しない。設定要求がPendingを返す前の失敗では構造化要求値はnullだが、レート読出し後のsetter等の失敗理由に値を残す。回帰試験で48k要求/準備完了の値、Pending消去後にも失敗履歴へ48kを保持することを確認。Clippyと最低Rust全workspace全ターゲットcheckが通過。実機最新版のレート追従と長時間通話、版情報の統合は未完了である。

## 診断のアプリ版とプロトコル識別（2026-10-01）

ReportMetadataとsave_new_with_metadataを追加し、GUIの保存処理が実行中アプリのCARGO_PKG_VERSION、main NSBundleのCFBundleShortVersionString/CFBundleVersionを取得して渡す。Cargoの0.5.0-alpha.1とBundleの0.5.1/build 2を同じ版へ統合せず、それぞれの出典に沿って保存する。汎用save_new/CLIで版が与えられない場合はnullを維持する。metadataにも既存64 KiBのserialize制限を適用する。

DriverReading::Availableには実際にdecoderが検証したdiagnostic_protocol=1とloaded_driver_version=nullを追加。現在のcbstは版付き統計のみで読み込み済みバイナリの製品版/ビルド識別を返さないため、設置済みInfo.plistを実ロードの版へ代用しない。新しいlive identityプロパティを加えて実HALで検証する作業は未完了であり、本変更は実ロード版の特定成功とは扱わない。保存回帰試験で未供給版nullと異なるCargo/Bundle/buildをそのまま保持することを確認。Clippyと最低Rust全workspace全ターゲットcheckが通過。実機ロード/長時間試験とproduction切替も未完了である。

## live driverのCargo版プロパティ（2026-10-01）

driverのDEVICEにread-only cbidを追加。生ASCII/non-NULのCARGO_PKG_VERSION（1..64 bytes）を返し、custの記述子をcbstとcbidの2件に拡張した。既存統計wire/protocol 1は変更しない。コンパイル時に版文字列長の上限を確認する。制御側は試験UIDだけで取得し、空/上限超過/非ASCII/改行/NULを拒否する。統計取得成功時に版取得を試み、JSONのloaded_driver_package_versionとidentity_errorへ結果を保存する。版だけ取得失敗しても統計をゼロや全体失敗へ置換しない。

これはHALが読んだlive codeのCargo版であり、設置済みplistを参照しない。一方、Cargo版を上げない修正間を識別するcommit/build hashではなく、Bundle 0.5.1との同一性や最新コミットのロード証明にはならない。旧loaded_driver_versionフィールドを意味の明確なloaded_driver_package_versionへ変更した。decoder試験でpre-release/build文字列保持と不正入力拒否を確認。Apple C ABIの模擬HALでcustの2件/各raw descriptor、cbidのsizeと正確な0.5.0-alpha.1 bytes、従来cbstの48 bytes/protocol 1を確認。最低Rust全workspace全ターゲットcheckと全体スクリプトが通過。実際の設置/再読み込み後のcbid取得は未完了であり、通常版移行と長時間ゲートも継続する。

## 診断改修をまとめた試験版0.5.2（2026-10-01）

全Rust cratesを0.5.2-alpha.1、試験アプリ/ドライバのBundleを0.5.2/build 3へ更新。模擬HALのlive identity期待値も0.5.2-alpha.1へ更新した。この版には互換性ゲート、入力/出力/FIFO/時計診断、再接続・要求レート履歴、診断保存GUI、live cbidを含む。通常Go版とproduction HALのID/ファイルは変更しない。

Clippy、最低Rust全workspace全ターゲットcheck、全体スクリプト、Universal driver/Appビルド、C模擬HAL、署名/metadata、空Cargoキャッシュで同梱ソースoffline/locked check、統合pkg展開後の両署名と実行ファイル/ソースarchive一致が通過。PackageInfoの実bundle2件が0.5.2/build 3、receiptがlocal.channelbridge.rust.test.pkg/0.5.2であることも確認した。初回XML検査はupgrade参照用のbundle要素まで版を要求して失敗したため、path属性を持つ実bundle2件を対象に検証した。pkgbuildのPermission denied警告は残っており、ペイロード一致検証の成功と原因解決を区別する。work/rust-dist/ChannelBridge-Rust-Test.pkgを生成したが、この時点では実導入/再読み込み/長時間通話の合格とは扱わない。

## 0.5.2統合インストーラの開始（2026-10-01）

既存の試験導入許可に沿ってFinderから検証済みChannelBridge-Rust-Test.pkgを開き、InstallerのURLが同パッケージであることを確認した。標準インストール先Macintosh HD/14.7 MBを確認後、インストールを押した。主ウインドウは暗くなり、17:15:40にSecurityAgent PID29984が起動した。17:16時点で統合receipt local.channelbridge.rust.test.pkgは未作成であり、導入完了とは扱わず管理者認証待ちと判断する。パスワードを取得/入力せず、ユーザーがシステムの認証画面へ直接入力するよう案内した。認証後にreceipt/bundle/署名/実行ファイル一致を確認し、設置とliveロードを別に検証する必要がある。

## 通常版と試験版のビルド識別（2026-10-01）

17:17にInstaller/SecurityAgentの同じPIDが生存し、統合receipt未作成を再確認。認証を代行せず独立した移行実装を進めた。contractのDeploymentに機器UID/名前、driver/app Bundle ID、アプリ名、設定ディレクトリを集約。defaultは既存試験版、production featureは実際の通常版local.channelbridge.app/local.channelbridge.driver/ChannelBridge_UIDとChannelBridge設定先を選ぶ。app/macOS/driverへfeatureを伝播し、アプリの表示名/Bundle検証/保存先、HAL公開UID/名前/ID、セッション対象/交渉/健全性/診断の対象を統一した。通常版IDは設置済みplistと既存driver/Config.hの実値を確認した。

試験用RateFollow::request_testとloopback/monitorは試験UIDを明示したままとし、製品経路はrequest_targetを使う。診断関数はread_target_driverへ改名した。旧Go形式の設定は両profileで読み込み、読出しによって原ファイルを書換えない。回帰試験で名前空間の分離/旧通常版ID維持と、両featureの設定先/チャンネル9・16維持/原ファイル不変を確認。通常/production contract試験、macos 19試験、default Clippy、最低Rust production全ターゲットcheck、Intel production全ターゲットcheck、default全体スクリプトが通過。通常版のplist/Universal配布ビルダー/移行と復帰の導入試験はまだ必要である。productionの実起動/インストール/設定更新はしておらず、認証待ちの既に生成済み0.5.2試験pkgは書き換えていない。長時間試験と署名/公証も未完了である。


### 2026-10-01 production Universal build and installed test receipt

Shared app/driver builders now accept --production and select dedicated plists,
output paths and Cargo artifacts. Production plists retain the existing app/driver
IDs, UID via the contract, and legacy factory UUID. Default test builds keep their
separate namespace. Unknown/extra driver options fail rather than silently choosing
a profile. Empty feature arrays work with Bash nounset. The source verifier accepts
an explicit production profile and checks all workspace targets offline from its
extracted archive with an empty Cargo cache.

Production arm64/x86_64 builds, ad-hoc signature checks, bundle validation and
non-audio AppKit smoke passed. The production driver passed its real C factory and
property/audio/rate/epoch tests against fake HAL; the default test driver regression
also passed. The final production app archive rebuilt offline with production
features. No production installation, real production app launch or settings update
was performed; pending test installer contents were not rewritten.

At 20:30 local time, pkgutil reports local.channelbridge.rust.test.pkg version 0.5.2
installed. coreaudiod and the test driver host still began at 12:00:42, before this
installation. A fresh read-only probe --diagnostics still returns HAL error
2003332927. Thus the installed receipt proves deployment, not live cbid/cbst loading;
reload/reboot and actual-driver verification remain necessary. Long-call, real rate
following, production migration/rollback and signing/notarization gates remain open.


### 2026-10-01 production package preparation

Added a dedicated production package builder and component plist. It rebuilds the
production Universal app/driver, verifies the embedded archive offline with the
production feature, then expands the package and verifies that all payload paths
match the staged app/driver tree. Strict signatures, both executable bytes and the
source archive are compared after expansion. No installer Scripts directory is
allowed. This generates a local ad-hoc package only; no production replacement or
audio-service restart is performed. RUST_MIGRATION.md records actual legacy backup,
restoration and live-driver verification requirements. Actual migration, uninstall
and rollback tests remain incomplete; real-device gates still require the newly
installed test driver to load. Existing coreaudiod/test host start times remain
12:00:42.

Production package build/expand checks passed. pkgbuild still emits `write: Permission denied` warnings as in the test package build; their cause is unresolved and payload verification is not treated as resolution of those warnings.


### 2026-10-01 package warning isolation and full payload integrity

An empty root pkgbuild invocation (no app, driver, component plist or source archive)
also emitted four identical write: Permission denied messages and exited zero.
This narrows the warning to the local packaging invocation/environment rather than
these bundle contents; the precise failed write and cause are still unknown. No
permission or system protection changes were attempted.

Production package expansion now compares every staged entry against the payload:
exact path set, file type, permission mode, symlink target and SHA-256 for every
regular file. The complete actual production package passed, in addition to strict
codesign and the executable/archive comparisons. This does not prove installation
or real-driver loading. Live audio acceptance and actual rollback remain pending.


### 2026-10-01 production CI coverage

CI now checks all targets for both deployment profiles on Apple Silicon and Intel,
adds production-feature workspace tests to the minimum-Rust lane, and builds the
production package in the current-Rust lane after the separate test package.
Timeout increased to 40 minutes for two Universal/offline package builds. Local
production workspace tests and production all-target Clippy passed. Workflow YAML
and presence of the production package step were parsed/verified. Remote workflow
execution is not verified by these checks (gh unavailable on this shell PATH);
GitHub CI success and billing availability remain unproven. Actual long-call and
production migration/rollback gates are unchanged and still incomplete.


### 2026-10-01 actual legacy backup

Added backup-rust-migration.py, requiring the actual legacy app 0.4.0 / driver 0.1.0
IDs and versions and a separate new destination. It copies bundles without system
writes, verifies every entry/mode/link/hash and unchanged source snapshots, and
verifies settings bytes without printing their contents. Private destination 0700,
settings/manifest 0600. An actual verified backup exists locally at ignored
work/legacy-backup-20261001, retaining the app from outputs/ChannelBridge.app,
installed legacy HAL and current production settings. This preserves rollback
artifacts; it is not an actual successful restore, migration or uninstall test.


### 2026-10-01 live-driver readiness gate

Added probe --ready for a read-only preflight: unique selected profile UID via
property lookup, live cbid matching the probe Cargo package, protocol-v1 decoded
cbst and nonzero generation. It opens no audio and does not request microphone
permission or change defaults. Clippy passed. Actual invocation failed with
HAL 2003332927; coreaudiod/test host still began at 12:00:42. Therefore actual new
live-driver readiness remains unproven. Asked the user to report completion of a
normal Mac reboot after the installed test update. A successful future preflight
will not establish exact Git revision, signal quality or long-call acceptance.

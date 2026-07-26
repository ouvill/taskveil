// ignore: unused_import
import 'package:intl/intl.dart' as intl;
import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for Japanese (`ja`).
class AppLocalizationsJa extends AppLocalizations {
  AppLocalizationsJa([String locale = 'ja']) : super(locale);

  @override
  String get appTitle => 'Taskveil';

  @override
  String get defaultInboxName => 'インボックス';

  @override
  String get defaultListMissing =>
      '既定リストが見つかりません。Taskveilを再起動するか、ローカルDBの初期化状態を確認してください。';

  @override
  String get listsTitle => 'リスト';

  @override
  String get listsSectionTitle => 'リスト';

  @override
  String get listsEmpty => 'リストがありません。+をタップして作成してください。';

  @override
  String get listsEmptyTitle => 'リストがありません。';

  @override
  String get listsEmptyBody => '+をタップして作成してください。';

  @override
  String failedToLoadLists(String error) {
    return 'リストの読み込みに失敗しました: $error';
  }

  @override
  String get newListTooltip => '新しいリスト';

  @override
  String get newListTitle => '新しいリスト';

  @override
  String get listActionsTooltip => 'リスト操作';

  @override
  String get listsMoreMenuTooltip => 'その他';

  @override
  String get renameListMenuItem => '名称変更';

  @override
  String get renameListTitle => 'リスト名を変更';

  @override
  String get archiveListMenuItem => 'アーカイブ';

  @override
  String get deleteListMenuItem => '削除';

  @override
  String get unarchiveListMenuItem => 'アーカイブ解除';

  @override
  String deleteListDialogTitle(String listName) {
    return '「$listName」を削除しますか？';
  }

  @override
  String deleteListDialogMessage(int taskCount) {
    return 'このリストは完全に削除され、元に戻せません。配下のタスク $taskCount件（完了済みを含む）はリマインダーや履歴を保持したままインボックスへ移動します。';
  }

  @override
  String archivedListsSectionTitle(int count) {
    return 'アーカイブ済み（$count件）';
  }

  @override
  String get showArchivedListsTooltip => 'アーカイブ済みリストを表示';

  @override
  String get hideArchivedListsTooltip => 'アーカイブ済みリストを隠す';

  @override
  String get nameLabel => '名前';

  @override
  String get cancelButton => 'キャンセル';

  @override
  String get deleteButton => '削除';

  @override
  String get createButton => '作成';

  @override
  String get tasksTitle => 'タスク';

  @override
  String get homeTitle => 'ホーム';

  @override
  String get calendarTitle => 'カレンダー';

  @override
  String get calendarWeekTab => '週';

  @override
  String get calendarMonthTab => '月';

  @override
  String get calendarPreviousPeriodTooltip => '前の期間';

  @override
  String get calendarNextPeriodTooltip => '次の期間';

  @override
  String get calendarGoToToday => '今日';

  @override
  String calendarSelectedDaySemantics(String date) {
    return '選択中の日付: $date';
  }

  @override
  String calendarDayTaskCount(int count) {
    return 'タスク $count件';
  }

  @override
  String get calendarCompletedTitle => '完了済み';

  @override
  String get calendarShowCompletedTooltip => '完了した成果を表示';

  @override
  String get calendarHideCompletedTooltip => '完了した成果を隠す';

  @override
  String get calendarEmptyTitle => '予定はありません。';

  @override
  String get calendarEmptyBody => '別の日を選ぶか、時間を使いたいことを追加できます。';

  @override
  String get calendarLoadFailed => 'カレンダーを読み込めませんでした。';

  @override
  String get calendarLoadingSemantics => 'カレンダーを読み込み中';

  @override
  String get calendarRetryButton => '再試行';

  @override
  String get calendarDueKind => '期限';

  @override
  String get calendarScheduledKind => '予定';

  @override
  String get calendarCompletedKind => '完了';

  @override
  String calendarArchivedListContext(String listName) {
    return '$listName · アーカイブ済み';
  }

  @override
  String calendarOccurrenceSemantics(
    String title,
    String listName,
    String kind,
    String time,
  ) {
    return '$title、$listName、$kind、$time';
  }

  @override
  String get calendarMoveDueTooltip => '期限日を移動';

  @override
  String get calendarMoveScheduledTooltip => '予定日を移動';

  @override
  String calendarMoveOccurrenceSemantics(String kind, String title) {
    return '$kind: $titleの日付を変更';
  }

  @override
  String get calendarMoveSheetTitle => '日付を変更';

  @override
  String get calendarMoveToToday => '今日';

  @override
  String get calendarMoveToTomorrow => '明日';

  @override
  String get calendarPickDate => '日付を選択…';

  @override
  String get todayTitle => '今日';

  @override
  String get homeOverdueSectionTitle => '期限超過';

  @override
  String get homeTomorrowSectionTitle => '明日';

  @override
  String get homeUpcomingSectionTitle => '今後';

  @override
  String get homeTasksSectionTitle => 'タスク';

  @override
  String homePendingCount(int count) {
    return '未完了 $count件';
  }

  @override
  String get completedTasksTitle => 'クローズ済み';

  @override
  String get showCompletedTasksTooltip => 'クローズ済みタスクを表示';

  @override
  String get hideCompletedTasksTooltip => 'クローズ済みタスクを隠す';

  @override
  String get homeListMenuTooltip => 'リストを開く';

  @override
  String get homeSmartListTooltip => 'ホームを開く';

  @override
  String get openSearchTooltip => 'タスクを検索';

  @override
  String get searchFieldHint => 'タスクとノートを検索';

  @override
  String get searchFieldSemantics => 'タスクとノートを検索';

  @override
  String get clearSearchTooltip => '検索をクリア';

  @override
  String get searchEmptyTitle => '必要なものを探す。';

  @override
  String get searchEmptyBody => 'すべてのリストからタスクのタイトルとノートを検索できます。';

  @override
  String get searchNoResultsTitle => '見つかりませんでした。';

  @override
  String searchNoResultsBody(String query) {
    return '「$query」に一致するタスクはありません。';
  }

  @override
  String get searchFailed => '検索できませんでした。';

  @override
  String get searchLoadingSemantics => 'タスクを検索中';

  @override
  String searchArchivedListLabel(String listName) {
    return '$listName · アーカイブ済み';
  }

  @override
  String searchResultSemantics(String title, String listName, String status) {
    return '$title、$listName、$status';
  }

  @override
  String showHomeSectionTooltip(String section) {
    return '$sectionのタスクを表示';
  }

  @override
  String hideHomeSectionTooltip(String section) {
    return '$sectionのタスクを隠す';
  }

  @override
  String get homeEmptyTitle => 'まずリストを作成';

  @override
  String get homeEmptyBody => 'リストを作成すると、次回からすぐタスクに入れます。';

  @override
  String get homeNewListButton => '新しいリスト';

  @override
  String get homeClearTitle => 'すこし、余白ができました。';

  @override
  String get homeClearBody => 'いま待っているタスクはありません。必要になったら、ここに追加できます。';

  @override
  String get addTaskButton => 'タスクを追加';

  @override
  String get quickAddHint => 'タスクを追加';

  @override
  String get quickAddOpenTooltip => 'タスク作成を開く';

  @override
  String get quickAddOpenSemantics => 'タスク作成シートを開く';

  @override
  String get quickAddSubmitTooltip => 'タスクを追加';

  @override
  String get quickAddTextFieldSemantics => 'クイック追加するタスクのタイトル';

  @override
  String get quickAddCreateError => 'タスクを追加できませんでした。';

  @override
  String get taskCreateTitleHint => 'タスクを追加...';

  @override
  String get taskCreateListChip => 'リスト';

  @override
  String get taskCreateListTooltip => 'リストを選択';

  @override
  String get taskCreateDueChip => '期限';

  @override
  String get taskCreateDueTooltip => '期限日を選択';

  @override
  String taskCreateDueChipSemantics(String dueAt) {
    return '期限: $dueAt';
  }

  @override
  String get taskCreatePlanLabel => '予定';

  @override
  String get taskCreatePlanTooltip => '開始予定と見積時間を設定';

  @override
  String get taskCreatePriorityTooltip => '優先度を選択';

  @override
  String get planNotSet => '予定なし';

  @override
  String get planSheetTitle => '予定';

  @override
  String get plannedStartLabel => '開始予定';

  @override
  String get setPlannedStartButton => '日時を設定';

  @override
  String get estimateLabel => '見積時間';

  @override
  String get estimateNotSet => '見積なし';

  @override
  String estimateMinutes(int minutes) {
    return '$minutes分';
  }

  @override
  String get decreaseEstimateTooltip => '見積時間を5分減らす';

  @override
  String get increaseEstimateTooltip => '見積時間を5分増やす';

  @override
  String get clearPlanButton => '予定をクリア';

  @override
  String get planSaveButton => '予定を適用';

  @override
  String get prioritySheetTitle => '優先度';

  @override
  String selectedOptionSemantics(String option) {
    return '選択中: $option';
  }

  @override
  String get tasksEmpty => 'タスクがありません。+をタップして作成してください。';

  @override
  String get tasksEmptyTitle => 'タスクがありません。';

  @override
  String get tasksEmptyBody => '+をタップして作成してください。';

  @override
  String failedToLoadTasks(String error) {
    return 'タスクの読み込みに失敗しました: $error';
  }

  @override
  String get newTaskTooltip => '新しいタスク';

  @override
  String get newTaskTitle => '新しいタスク';

  @override
  String get titleLabel => 'タイトル';

  @override
  String get noteLabel => 'ノート';

  @override
  String get taskDetailTitle => 'タスク詳細';

  @override
  String failedToLoadTask(String error) {
    return 'タスクの読み込みに失敗しました: $error';
  }

  @override
  String get taskNotFound => 'タスクが見つかりません。';

  @override
  String taskPriority(String priority) {
    return '優先度: $priority';
  }

  @override
  String taskDueAt(String dueAt) {
    return '$dueAt';
  }

  @override
  String taskRowStatusSemantics(String status) {
    return '状態: $status';
  }

  @override
  String taskRowDueSemantics(String dueAt) {
    return '期限: $dueAt';
  }

  @override
  String taskRowListSemantics(String listName) {
    return 'リスト: $listName';
  }

  @override
  String taskRowSubtaskLevelSemantics(int level) {
    return 'サブタスク階層 $level';
  }

  @override
  String get taskRowOpenHint => 'ダブルタップでタスクを開く';

  @override
  String get dueToday => '今日';

  @override
  String get dueTomorrow => '明日';

  @override
  String taskDueOverdue(String dueAt) {
    return '期限超過: $dueAt';
  }

  @override
  String taskCreatedAt(String createdAt) {
    return '作成日時: $createdAt';
  }

  @override
  String get addNotePlaceholder => 'ノートを追加';

  @override
  String get editTaskTitleSemantics => 'タスクのタイトルを編集';

  @override
  String get editTaskNoteSemantics => 'タスクのノートを編集';

  @override
  String parentTaskLinkTooltip(String title) {
    return '親タスクを開く: $title';
  }

  @override
  String parentTaskLinkSemantics(String title) {
    return '親タスク: $title';
  }

  @override
  String get changeDueDateTooltip => '期限日を変更';

  @override
  String get reminderChipEmpty => 'リマインダー';

  @override
  String get reminderChipTooltipSet => 'リマインダーを設定';

  @override
  String get reminderChipTooltipChange => 'リマインダーを管理';

  @override
  String get clearReminderButton => 'リマインダーを解除';

  @override
  String get manageRemindersTitle => 'リマインダー';

  @override
  String get addReminderButton => 'リマインダーを追加';

  @override
  String get editReminderTooltip => 'リマインダーを編集';

  @override
  String get deleteReminderTooltip => 'リマインダーを削除';

  @override
  String reminderCount(int count) {
    return '$count件のリマインダー';
  }

  @override
  String get reminderQuickOptionsTitle => '期限時刻より前';

  @override
  String get reminderFiveMinutesBefore => '5分前';

  @override
  String get reminderThirtyMinutesBefore => '30分前';

  @override
  String get reminderOneHourBefore => '1時間前';

  @override
  String get reminderOneDayBefore => '1日前';

  @override
  String get reminderCustomTime => '日時を指定';

  @override
  String get reminderPastLabel => '過去';

  @override
  String get reminderLimitReached => '1つのタスクには最大5件まで設定できます。';

  @override
  String get reminderDuplicateTime => '同じ時刻のリマインダーがすでにあります。';

  @override
  String get reminderMustBeFuture => '未来の時刻を選んでください。';

  @override
  String get reminderSavedNotificationFailed =>
      'リマインダーは保存しましたが、Taskveilはローカル通知を登録できませんでした。';

  @override
  String get reminderPermissionDenied =>
      '通知がオフです。リマインダーは保存しましたが、Taskveilはローカル通知を登録できませんでした。';

  @override
  String failedToSaveReminder(String error) {
    return 'リマインダーの保存に失敗しました: $error';
  }

  @override
  String get reminderNotificationTitle => 'Taskveil リマインダー';

  @override
  String get reminderNotificationBody => 'タスクのリマインダー時刻です。';

  @override
  String get reminderSnoozeOneHourAction => '1時間後';

  @override
  String get changePriorityTooltip => '優先度を変更';

  @override
  String get subtasksTitle => 'サブタスク';

  @override
  String get subtasksEmpty => 'サブタスクはまだありません。';

  @override
  String get addSubtaskButton => 'サブタスクを追加';

  @override
  String get newSubtaskTitle => '新しいサブタスク';

  @override
  String subtaskProgress(int doneCount, int totalCount) {
    return '$doneCount/$totalCount';
  }

  @override
  String get completeTaskDialogTitle => '親タスクを完了しますか？';

  @override
  String get completeTaskDialogMessage =>
      'このタスクには未完了のサブタスクがあります。親タスクを完了しても、サブタスクは自動では完了しません。';

  @override
  String get wontDoTaskDialogTitle => '親タスクを対応しないとして閉じますか？';

  @override
  String get wontDoTaskDialogMessage =>
      'このタスクには未完了のサブタスクがあります。親タスクを対応しないとして閉じても、サブタスクは自動では閉じられません。';

  @override
  String get continueButton => '続行';

  @override
  String get statusTodo => '未着手';

  @override
  String get statusInProgress => '進行中';

  @override
  String get statusDone => '完了';

  @override
  String get statusWontDo => '対応しない';

  @override
  String get editTaskTooltip => 'タスクを編集';

  @override
  String get taskActionsTooltip => 'タスク操作';

  @override
  String get completeTaskTooltip => 'タスクを完了にする';

  @override
  String get markTaskDoneMenuItem => '完了にする';

  @override
  String get markTaskWontDoMenuItem => '対応しない';

  @override
  String get reopenTaskTooltip => 'タスクを再オープン';

  @override
  String get reopenTaskMenuItem => '再オープン';

  @override
  String get editTaskTitle => 'タスクを編集';

  @override
  String get priorityLabel => '優先度';

  @override
  String get priorityNone => 'なし';

  @override
  String get priorityLow => '低';

  @override
  String get priorityMedium => '中';

  @override
  String get priorityHigh => '高';

  @override
  String get dueDateLabel => '期限';

  @override
  String get noDueDate => '期限なし';

  @override
  String get setDueDateButton => '日付を設定';

  @override
  String get setDueDateTimeButton => '日時を設定';

  @override
  String get clearDueDateButton => '日付をクリア';

  @override
  String get saveButton => '保存';

  @override
  String get titleRequiredError => 'タイトルは必須です。';

  @override
  String failedToSaveTask(String error) {
    return 'タスクの保存に失敗しました: $error';
  }

  @override
  String get deleteTaskMenuItem => '削除';

  @override
  String get deleteTaskDialogTitle => 'タスクを削除しますか？';

  @override
  String get deleteTaskDialogMessage => 'このタスクは完全に削除され、元に戻せません。';

  @override
  String deleteTaskDialogMessageWithDescendants(int descendantCount) {
    return 'このタスクと配下のサブタスク $descendantCount件は完全に削除され、元に戻せません。';
  }

  @override
  String get undoActionLabel => '元に戻す';

  @override
  String get undoCompleteMessage => 'タスクを完了しました。';

  @override
  String get undoCloseMessage => 'タスクを閉じました。';

  @override
  String get undoEditMessage => 'タスクを保存しました。';

  @override
  String get undoSuccessMessage => '元に戻しました。';

  @override
  String undoFailedMessage(String error) {
    return '元に戻せませんでした: $error';
  }

  @override
  String get taskSortTooltip => 'タスクの表示順';

  @override
  String get taskSortManual => '手動順';

  @override
  String get taskSortDueDate => '締切順';

  @override
  String get taskSortPriority => '優先度順';

  @override
  String get taskSortCreatedAt => '作成順';

  @override
  String get moveTaskUpTooltip => 'タスクを上へ移動';

  @override
  String get moveTaskDownTooltip => 'タスクを下へ移動';

  @override
  String get accountTitle => 'アカウント';

  @override
  String get accountSubtitle => 'プライベート同期・セキュリティ・アカウントの設定。';

  @override
  String get navigationMenuLabel => 'メニュー';

  @override
  String get menuTitle => 'メニュー';

  @override
  String get menuSubtitle => 'ワークスペース、アカウント、繰り返し使うツール。';

  @override
  String get menuSectionTitle => 'ワークスペース';

  @override
  String get menuAccountBody => 'ログイン、同期、セキュリティ、Pro';

  @override
  String get menuTemplatesBody => '再利用タスクと繰り返しタスク';

  @override
  String get calendarSettingsTitle => 'カレンダー設定';

  @override
  String get calendarSettingsSubtitle => 'カレンダーで1週間をどの曜日から表示するか選びます。';

  @override
  String get calendarWeekStartSectionTitle => '週の始まり';

  @override
  String get calendarWeekStartSystem => '地域の設定に従う';

  @override
  String get calendarWeekStartSystemBody => 'この端末の地域設定に合わせます。';

  @override
  String get calendarWeekStartMonday => '月曜日';

  @override
  String get calendarWeekStartMondayBody => '月曜日を最初の列に表示します。';

  @override
  String get calendarWeekStartSunday => '日曜日';

  @override
  String get calendarWeekStartSundayBody => '日曜日を最初の列に表示します。';

  @override
  String get calendarSettingsLoadFailed => 'カレンダー設定を読み込めませんでした。';

  @override
  String get calendarSettingsSaveFailed => 'カレンダー設定を保存できませんでした。もう一度お試しください。';

  @override
  String get accountPrivateSectionTitle => 'プライベートアカウント';

  @override
  String get accountPrivateTitle => '暗号化されたワークスペース';

  @override
  String get accountPrivateBody => '保護されたタスクを、端末間で同期できます。';

  @override
  String get accountEncryptionStatus => 'エンドツーエンド暗号化済み';

  @override
  String get accountSecurityTitle => 'セキュリティ';

  @override
  String get accountConnectionTitle => '接続';

  @override
  String get accountConnectionBody => 'この端末で使う同期サーバーの詳細設定です。';

  @override
  String get accountLoadFailed => 'アカウント状態を読み込めませんでした。';

  @override
  String get accountLoginTab => 'ログイン';

  @override
  String get accountRegisterTab => '登録';

  @override
  String get accountEmailLabel => 'メール';

  @override
  String get accountPasswordLabel => 'パスワード';

  @override
  String get accountLoginButton => 'ログイン';

  @override
  String get accountRegisterButton => '登録';

  @override
  String get accountVerificationPending =>
      'メールで届いた8桁の確認コードを入力してください。確認できるのはメールボックスへのアクセスだけで、暗号化データの復旧には使用できません。';

  @override
  String get accountVerificationCodeLabel => '8桁の確認コード';

  @override
  String get accountVerifyCodeButton => 'コードを確認';

  @override
  String get accountResendCodeButton => 'コードを再送';

  @override
  String accountResendAvailableIn(int seconds) {
    return '$seconds秒後に再送できます。';
  }

  @override
  String get accountCancelRegistrationButton => '登録を中止';

  @override
  String get accountSetPasswordButton => 'パスワードを設定して完了';

  @override
  String get accountRecoveryAcknowledgeButton => 'Recovery Keyを保存しました';

  @override
  String get accountLogoutButton => 'ログアウト';

  @override
  String get accountServerUrlLabel => 'サーバーURL';

  @override
  String get accountSaveServerUrlTooltip => 'サーバーURLを保存';

  @override
  String get accountRequestFailed => 'アカウント処理に失敗しました。';

  @override
  String get accountSyncTitle => '同期';

  @override
  String get accountSyncNotSignedIn => '同期はオフです。';

  @override
  String get accountSyncIdle => '待機中';

  @override
  String get accountSyncRunning => '同期中';

  @override
  String get accountSyncFailed => '同期に失敗しました';

  @override
  String accountSyncLastSuccess(String time) {
    return '最終同期: $time';
  }

  @override
  String get accountSyncNever => '未同期';

  @override
  String get accountSyncNowButton => '今すぐ同期';

  @override
  String get accountSyncNowTooltip => '今すぐ同期';

  @override
  String get organizationSafetyOpenButton => '組織メンバーを確認';

  @override
  String get organizationSafetyTitle => 'Safety number';

  @override
  String get organizationSafetyBody =>
      '別の信頼できる経路で相手とこの番号またはQRコードを照合してください。両アカウントが同じ値を確認するまで、組織の鍵は配送されません。';

  @override
  String get organizationTenantIdLabel => '組織ID';

  @override
  String get organizationMemberIdLabel => 'メンバーのアカウントID';

  @override
  String get organizationSafetyLoadButton => 'Safety numberを表示';

  @override
  String get organizationSafetyQrSemantics => 'Safety numberのQRコード';

  @override
  String get organizationSafetyVerified => '両アカウントで確認済み';

  @override
  String get organizationSafetyUnverified => '両アカウントでの確認が未完了です';

  @override
  String get organizationSafetyComparedOutOfBand => '別の信頼できる経路でこの値を照合しました';

  @override
  String get organizationSafetyConfirmButton => 'このSafety numberを確認';

  @override
  String get organizationSafetyFailed =>
      'Safety numberを確認できませんでした。再読み込みして、もう一度照合してください。';

  @override
  String get onboardingWelcomeTitle => '大切なことに、余白を';

  @override
  String get onboardingWelcomeBody =>
      '予定も、小さな約束も、次に向き合うことも。点数や騒がしさのない、静かな居場所です。';

  @override
  String get onboardingWelcomeArtworkSemantics => 'Taskveilを表す静かな葉';

  @override
  String get onboardingPrivacyTitle => 'プライバシーを守る';

  @override
  String get onboardingPrivacyBody =>
      'ローカルデータベースは、この端末上で暗号化されます。同期を選んだ場合、タスク内容は端末を離れる前に暗号化されます。';

  @override
  String get onboardingPrivacyNote =>
      '同期しないタスクはこの端末だけに保存されます。端末の紛失やアプリの削除により、復旧できなくなる場合があります。';

  @override
  String get onboardingPrivacyArtworkSemantics => 'ローカル保護と暗号化同期を表す盾';

  @override
  String get onboardingBeginTitle => 'まず、ひとつだけ';

  @override
  String get onboardingBeginBody =>
      '気になっていることを追加しましょう。必要なときまで、Taskveilは静かに待っています。';

  @override
  String get onboardingBeginArtworkSemantics => '静かな完了を表すチェックマーク';

  @override
  String get onboardingStartButton => 'そっと始める';

  @override
  String get onboardingSaveFailed => 'この選択を保存できませんでした。もう一度お試しください。';

  @override
  String get onboardingLoadFailed => 'ローカル設定を読み込めませんでした。';

  @override
  String get retryButton => 'もう一度試す';

  @override
  String onboardingPagePosition(int current, int total) {
    return '$totalページ中$currentページ';
  }

  @override
  String get focusTitle => '集中';

  @override
  String get focusSetupTitle => '集中のしかたを選ぶ';

  @override
  String get focusSetupBody => 'ひとつのタスクに向き合う時間です。ほかのことは、いったん置いておきましょう。';

  @override
  String get focusPomodoroMode => 'ポモドーロ';

  @override
  String get focusStopwatchMode => 'ストップウォッチ';

  @override
  String focusPomodoroSummary(int work, int breakMinutes) {
    return '集中 $work分・休憩 $breakMinutes分';
  }

  @override
  String get focusStopwatchSummary => '終了時刻を決めず、休止と再開ができます';

  @override
  String get focusStartButton => '集中を始める';

  @override
  String get focusSettingsButton => 'ポモドーロ設定';

  @override
  String get focusSettingsTitle => 'ポモドーロのリズム';

  @override
  String get focusWorkMinutesLabel => '集中';

  @override
  String get focusShortBreakMinutesLabel => '短い休憩';

  @override
  String get focusLongBreakMinutesLabel => '長い休憩';

  @override
  String get focusLongBreakEveryLabel => '長い休憩まで';

  @override
  String get focusNotificationsLabel => '終了時に通知';

  @override
  String get focusNotificationsBody => 'Taskveilがバックグラウンド中は可能な範囲で通知します';

  @override
  String focusWorkIntervals(int count) {
    return '集中 $count回';
  }

  @override
  String get focusRestoring => '集中セッションを復元しています…';

  @override
  String get focusLoadFailed => '集中セッションを復元できませんでした。';

  @override
  String get focusActiveConflictTitle => '別の集中セッションが進行中です';

  @override
  String get focusActiveConflictBody => '現在のセッションを終了または破棄してから、このタスクを始めてください。';

  @override
  String get focusRunningState => '集中しています';

  @override
  String get focusPausedState => '一時停止中';

  @override
  String get focusWorkPhase => '集中セッション';

  @override
  String get focusShortBreakPhase => '短い休憩';

  @override
  String get focusLongBreakPhase => '長い休憩';

  @override
  String get focusBreakPrompt => 'ひと息つきましょう。';

  @override
  String focusElapsedLabel(String time) {
    return '経過 $time';
  }

  @override
  String get focusPauseButton => '一時停止';

  @override
  String get focusResumeButton => '再開';

  @override
  String get focusSessionOptionsButton => 'セッションの操作';

  @override
  String get focusAddTimeButton => '5分追加';

  @override
  String get focusFinishButton => '終了';

  @override
  String get focusFinishSessionButton => 'セッションを終了';

  @override
  String get focusEndBreakButton => '休憩を終了';

  @override
  String get focusSaveAndExitButton => '保存して終了';

  @override
  String get focusDiscardButton => '破棄';

  @override
  String get focusDiscardTitle => 'このセッションを破棄しますか？';

  @override
  String get focusDiscardBody => '作業時間を保存せずにセッションを終了します。';

  @override
  String get focusCompleteTaskButton => 'タスクを完了';

  @override
  String get focusFinishedTitle => '集中時間を記録しました';

  @override
  String get focusFinishedSummary => '作業時間を保存しました。';

  @override
  String focusFinishedBody(String time) {
    return '$timeの作業時間を保存しました。';
  }

  @override
  String get focusBreakFinishedTitle => '休憩が終わりました';

  @override
  String get focusBreakFinishedBody => '準備ができたら、次の集中を始めましょう。';

  @override
  String get focusStartBreakButton => '休憩を始める';

  @override
  String get focusKeepSessionButton => '集中を続ける';

  @override
  String get focusDoneButton => '完了';

  @override
  String get focusActionFailed => '集中セッションを更新できませんでした。もう一度お試しください。';

  @override
  String get focusTaskCompleteFailed =>
      '集中時間は保存されましたが、タスクを完了できませんでした。もう一度タスクを完了してください。';

  @override
  String get focusEstimateActualLabel => '集中時間';

  @override
  String focusEstimateActualValue(String actual, String estimate) {
    return '実績 $actual・見積 $estimate';
  }

  @override
  String focusActualOnlyValue(String actual) {
    return '実績 $actual';
  }

  @override
  String get focusNoActualValue => '作業時間はまだありません';

  @override
  String get timerNotificationTitle => '集中時間が終わりました';

  @override
  String get timerNotificationBody => 'Taskveilを開いて、次のリズムへ進みましょう。';

  @override
  String get billingTitle => 'Pro';

  @override
  String get billingSubscriptionBody =>
      'ProではE2EE同期と暗号化クラウドバックアップを利用できます。端末のサブスクリプション設定からいつでも解約できます。';

  @override
  String get billingStatusFree => 'Free';

  @override
  String get billingStatusTrial => 'トライアル中';

  @override
  String get billingStatusActive => '利用中';

  @override
  String get billingStatusGrace => '支払い猶予期間中';

  @override
  String get billingStatusExpired => '期限切れ';

  @override
  String get billingStatusRevoked => '失効';

  @override
  String get billingMonthlyLabel => '月額';

  @override
  String get billingYearlyLabel => '年額';

  @override
  String get billingPurchaseButton => 'Proを始める';

  @override
  String get billingRestoreButton => '購入を復元';

  @override
  String get billingManageButton => 'サブスクリプションを管理';

  @override
  String get billingUnavailable => '現在、課金情報を取得できません。';

  @override
  String get billingCancelled => '購入をキャンセルしました。';

  @override
  String get billingPending => '購入を処理中です。アプリストアで支払いが確認されるとProが有効になります。';

  @override
  String get billingFailed => '購入を完了できませんでした。';

  @override
  String get billingRestored => '購入状態を更新しました。';

  @override
  String billingPriceSemantics(String period, String price) {
    return '$period、$price';
  }

  @override
  String get backButtonTooltip => '戻る';

  @override
  String get templatesTitle => 'テンプレート';

  @override
  String templatesLoadFailed(String error) {
    return 'テンプレートを読み込めませんでした: $error';
  }

  @override
  String get templatesEmptyTitle => 'テンプレートはまだありません';

  @override
  String get templatesEmptyBody => 'ここで定型タスクを作るか、既存タスクをテンプレートとして保存できます。';

  @override
  String get newTemplateButton => '新規テンプレート';

  @override
  String get newTemplateTitle => '新しいテンプレート';

  @override
  String get templateActionsTooltip => 'テンプレートの操作';

  @override
  String get editButton => '編集';

  @override
  String get replaceTemplateSnapshotMenuItem => '内容を置き換える';

  @override
  String templateTaskCount(int count) {
    return '$count件のタスク';
  }

  @override
  String get createFromTemplateButton => 'タスクを作成';

  @override
  String get addScheduleTooltip => '繰り返しタスクを追加';

  @override
  String get createTaskSeriesTooltip => 'このテンプレートから繰り返しタスクを作成';

  @override
  String get taskSeriesTitle => '繰り返しタスク';

  @override
  String get taskSeriesEmptyTitle => '繰り返しタスクはありません';

  @override
  String get taskSeriesEmptyBody => 'テンプレートから作成できます。作成後はテンプレートから独立します。';

  @override
  String get templateCreatedMessage => 'テンプレートからタスクを作成しました。';

  @override
  String get replaceTemplateSnapshotTitle => 'テンプレート内容を置き換える';

  @override
  String get duplicateTemplateMenuItem => '複製';

  @override
  String templateCopyName(String name) {
    return '$name（コピー）';
  }

  @override
  String get sourceTaskIdLabel => '元タスクID';

  @override
  String get replaceButton => '置き換える';

  @override
  String deleteTemplateDialogTitle(String name) {
    return '$nameを削除しますか？';
  }

  @override
  String get deleteTemplateDialogBody => 'このテンプレートから作成した繰り返しタスクとタスクは変更されません。';

  @override
  String get deleteScheduleDialogTitle => '繰り返しタスクを削除しますか？';

  @override
  String get deleteScheduleDialogBody => '作成済みタスクは変更されません。';

  @override
  String get scheduleEndedLabel => '終了済み';

  @override
  String scheduleSemantics(String rule, String next) {
    return '繰り返しタスク $rule、次回 $next';
  }

  @override
  String scheduleStreak(int count) {
    return '連続$count回';
  }

  @override
  String get scheduleActionsTooltip => '繰り返しタスクの操作';

  @override
  String get pauseScheduleMenuItem => '停止';

  @override
  String get resumeScheduleMenuItem => '再開';

  @override
  String get editTemplateTitle => 'テンプレートを編集';

  @override
  String get blueprintRootLabel => 'ルートタスク';

  @override
  String get blueprintChildLabel => '子タスク';

  @override
  String get addBlueprintChildButton => '子タスクを追加';

  @override
  String get moveUpButton => '上へ移動';

  @override
  String get moveDownButton => '下へ移動';

  @override
  String get defaultListLabel => '既定のリスト';

  @override
  String get inboxFallbackLabel => 'Inboxへフォールバック';

  @override
  String get newScheduleTitle => '新しい繰り返しタスク';

  @override
  String get editScheduleTitle => '繰り返しタスクを編集';

  @override
  String get schedulePresetLabel => '繰り返し';

  @override
  String get dailyPreset => '毎日';

  @override
  String get weeklyPreset => '毎週この曜日';

  @override
  String get monthlyPreset => '毎月この日';

  @override
  String get advancedPreset => '詳細RRULE';

  @override
  String get rruleLabel => 'RRULE';

  @override
  String get scheduleStartsAtLabel => '開始日時';

  @override
  String get timeZoneLabel => 'タイムゾーン';

  @override
  String get scheduleEnabledLabel => '繰り返しタスクを有効にする';

  @override
  String get editSeriesContentMenuItem => 'タスク内容と起票先を編集';

  @override
  String get editSeriesContentTitle => '繰り返しタスクの内容を編集';

  @override
  String get targetListLabel => '起票先リスト';

  @override
  String get scheduleValidationFailed => '繰り返しルール、開始日時、タイムゾーンを確認してください。';

  @override
  String get saveAsTemplateMenuItem => 'テンプレートとして保存';

  @override
  String get createTaskSeriesMenuItem => '繰り返しタスクにする';

  @override
  String get taskSeriesCreatedMessage => '繰り返しタスクを作成しました。';

  @override
  String get saveAsTemplateTitle => 'テンプレートとして保存';

  @override
  String get templateSavedMessage => 'テンプレートを保存しました。';

  @override
  String failedToStartTaskveil(String error) {
    return 'Taskveilの起動に失敗しました: $error';
  }
}

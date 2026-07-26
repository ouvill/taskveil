import 'dart:async';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:taskveil/src/billing/billing_store.dart';
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/core/civil_time.dart';
import 'package:taskveil/src/core/realtime_sync.dart';
import 'package:taskveil/src/core/task_tree.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/notifications/reminder_notifications.dart';
import 'package:taskveil/src/timer/timer_engine.dart';
import 'package:taskveil/src/timer/timer_notifications.dart';
import 'package:taskveil/src/timer/timer_settings.dart';
import 'package:taskveil/src/rust/api.dart'
    show
        AccountAuthResultDto,
        AccountSessionStateDto,
        BillingStateDto,
        CalendarOccurrenceDto,
        CalendarOccurrenceKindDto_Completed,
        CalendarOccurrenceKindDto_DateDue,
        CalendarOccurrenceKindDto_DateTimeDue,
        CalendarOccurrenceKindDto_Scheduled,
        CalendarRangeInput,
        CompletedTimerSessionDto,
        HomeTaskDto,
        ListDto,
        ReminderDto,
        SyncStatusDto,
        SyncNowOutcomeDto_BillingRequired,
        SyncNowOutcomeDto_Synced,
        TaskDto,
        TaskDueDto,
        TaskUndoDto,
        TimerFinishKindDto,
        TimerPhaseDto;

/// The [BridgeService] used by the app.
///
/// Defaults to [FrbBridgeService] (the real native bridge). Widget tests
/// override this with an in-memory fake via
/// `ProviderScope(overrides: [bridgeServiceProvider.overrideWithValue(fake)])`
/// so no test depends on the native Rust library or `initCore`.
final bridgeServiceProvider = Provider<BridgeService>(
  (ref) => const FrbBridgeService(),
);

/// Feature-scoped views of the aggregate bridge.
///
/// Each provider derives from [bridgeServiceProvider], so existing tests can
/// keep overriding the aggregate while feature code depends on a narrow port.
final accountBridgeProvider = Provider<AccountBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final syncBridgeProvider = Provider<SyncBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final billingBridgeProvider = Provider<BillingBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final listBridgeProvider = Provider<ListBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final templateBridgeProvider = Provider<TemplateBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final taskBridgeProvider = Provider<TaskBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final timerBridgeProvider = Provider<TimerBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final settingsBridgeProvider = Provider<SettingsBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final reminderBridgeProvider = Provider<ReminderBridgePort>(
  (ref) => ref.watch(bridgeServiceProvider),
);

final billingStoreProvider = Provider<BillingStore>(
  (ref) => RevenueCatBillingStore(),
);

final realtimeTimerFactoryProvider = Provider<RealtimeTimerFactory>(
  (ref) => systemRealtimeTimerFactory,
);

final realtimeSocketConnectorProvider = Provider<RealtimeSocketConnector>(
  (ref) => const IoRealtimeSocketConnector(),
);

final realtimeEventSinkProvider = Provider<RealtimeEventSink>(
  (ref) => systemRealtimeEventSink,
);

const taskSearchDebounceDuration = Duration(milliseconds: 250);

final taskSearchDebounceDurationProvider = Provider<Duration>(
  (ref) => taskSearchDebounceDuration,
);

sealed class TaskSearchState {
  const TaskSearchState();
}

final class TaskSearchIdle extends TaskSearchState {
  const TaskSearchIdle();
}

final class TaskSearchLoading extends TaskSearchState {
  const TaskSearchLoading(this.query);

  final String query;
}

final class TaskSearchData extends TaskSearchState {
  const TaskSearchData({required this.query, required this.items});

  final String query;
  final List<TaskSearchResult> items;
}

final class TaskSearchError extends TaskSearchState {
  const TaskSearchError({
    required this.query,
    required this.error,
    required this.stackTrace,
  });

  final String query;
  final Object error;
  final StackTrace stackTrace;
}

class TaskSearchResult {
  const TaskSearchResult({
    required this.task,
    required this.listName,
    required this.listArchived,
  });

  final TaskDto task;
  final String listName;
  final bool listArchived;
}

class TaskSearchNotifier extends Notifier<TaskSearchState> {
  Timer? _debounceTimer;
  var _generation = 0;
  var _disposed = false;

  @override
  TaskSearchState build() {
    ref.onDispose(() {
      _disposed = true;
      _generation += 1;
      _debounceTimer?.cancel();
    });
    return const TaskSearchIdle();
  }

  void setQuery(String value) {
    final query = value.trim();
    _debounceTimer?.cancel();
    if (query.isEmpty) {
      _generation += 1;
      state = const TaskSearchIdle();
      return;
    }

    _startSearch(query, debounce: true);
  }

  /// Re-runs the current non-empty query after a task, list, or sync mutation.
  ///
  /// Search results are snapshots rather than a derived task provider, so
  /// preserving and immediately refreshing the query prevents a detail edit
  /// from leaving a stale row behind when the user returns to Search.
  void refresh() {
    final query = switch (state) {
      TaskSearchLoading(:final query) => query,
      TaskSearchData(:final query) => query,
      TaskSearchError(:final query) => query,
      TaskSearchIdle() => null,
    };
    if (query != null) {
      _debounceTimer?.cancel();
      _startSearch(query, debounce: false);
    }
  }

  void _startSearch(String query, {required bool debounce}) {
    final generation = ++_generation;

    state = TaskSearchLoading(query);
    final delay = ref.read(taskSearchDebounceDurationProvider);
    if (!debounce || delay == Duration.zero) {
      unawaited(_search(query, generation));
      return;
    }
    _debounceTimer = Timer(delay, () {
      unawaited(_search(query, generation));
    });
  }

  void clear() => setQuery('');

  Future<void> _search(String query, int generation) async {
    final bridge = ref.read(bridgeServiceProvider);
    try {
      final taskFuture = bridge.searchTasks(query: query);
      final activeListsFuture = bridge.getLists();
      final archivedListsFuture = bridge.getArchivedLists();
      final tasks = await taskFuture;
      final activeLists = await activeListsFuture;
      final archivedLists = await archivedListsFuture;
      if (_disposed || generation != _generation) {
        return;
      }
      final listsById = {
        for (final list in activeLists) list.id: list,
        for (final list in archivedLists) list.id: list,
      };
      state = TaskSearchData(
        query: query,
        items: List.unmodifiable(
          tasks.map((task) {
            final list = listsById[task.listId];
            return TaskSearchResult(
              task: task,
              listName: list?.name ?? '',
              listArchived: list?.archivedAt != null,
            );
          }),
        ),
      );
    } catch (error, stackTrace) {
      if (_disposed || generation != _generation) {
        return;
      }
      state = TaskSearchError(
        query: query,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }
}

final taskSearchProvider =
    NotifierProvider<TaskSearchNotifier, TaskSearchState>(
      TaskSearchNotifier.new,
    );

const uiModeSettingKey = 'ui_mode';
const onboardingCompletedSettingKey = 'onboarding_completed';
const syncServerUrlSettingKey = 'sync_server_url';
const calendarWeekStartSettingKey = 'calendar_week_start';
const defaultSyncServerUrl = 'http://localhost:3000';
const defaultUiMode = 'simple';
const simpleUiMode = 'simple';
const advancedUiMode = 'advanced';
const defaultCalendarWeekStart = 'system';
const systemCalendarWeekStart = 'system';
const mondayCalendarWeekStart = 'monday';
const sundayCalendarWeekStart = 'sunday';
const _supportedUiModes = {simpleUiMode, advancedUiMode};
const _supportedCalendarWeekStarts = {
  systemCalendarWeekStart,
  mondayCalendarWeekStart,
  sundayCalendarWeekStart,
};

/// Thin typed entry point for app settings stored in the encrypted local DB.
///
/// The generic key/value methods are kept for future notification, theme, and
/// account settings. Feature-specific helpers own defaults and validation.
class SettingsRepository {
  SettingsRepository(this._bridge);

  final SettingsBridgePort _bridge;

  Future<String?> getSetting(String key) {
    return _bridge.getSetting(key: key);
  }

  Future<void> setSetting(String key, String value) {
    return _bridge.setSetting(key: key, value: value);
  }

  Future<String> getUiMode() async {
    final persisted = await getSetting(uiModeSettingKey);
    if (persisted == null || !_supportedUiModes.contains(persisted)) {
      return defaultUiMode;
    }
    return persisted;
  }

  Future<void> setUiMode(String uiMode) {
    if (!_supportedUiModes.contains(uiMode)) {
      throw ArgumentError.value(uiMode, 'uiMode', 'unsupported UI mode');
    }
    return setSetting(uiModeSettingKey, uiMode);
  }

  Future<String> getCalendarWeekStart() async {
    final persisted = await getSetting(calendarWeekStartSettingKey);
    if (persisted == null ||
        !_supportedCalendarWeekStarts.contains(persisted)) {
      return defaultCalendarWeekStart;
    }
    return persisted;
  }

  Future<void> setCalendarWeekStart(String weekStart) {
    if (!_supportedCalendarWeekStarts.contains(weekStart)) {
      throw ArgumentError.value(
        weekStart,
        'weekStart',
        'unsupported calendar week start',
      );
    }
    return setSetting(calendarWeekStartSettingKey, weekStart);
  }
}

class SyncServerUrlNotifier extends AsyncNotifier<String> {
  @override
  FutureOr<String> build() {
    return ref.watch(syncBridgeProvider).getSyncServerUrl();
  }

  Future<void> setServerUrl(String serverUrl) async {
    await ref.read(syncBridgeProvider).setSyncServerUrl(serverUrl: serverUrl);
    ref.invalidateSelf();
  }
}

final syncServerUrlProvider =
    AsyncNotifierProvider<SyncServerUrlNotifier, String>(
      SyncServerUrlNotifier.new,
    );

class AccountNotifier extends AsyncNotifier<AccountSessionStateDto> {
  @override
  FutureOr<AccountSessionStateDto> build() {
    return ref.watch(accountBridgeProvider).getAccountSessionState();
  }

  Future<AccountAuthResultDto> register({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    final result = await ref
        .read(accountBridgeProvider)
        .accountRegister(
          email: email,
          password: password,
          serverUrl: serverUrl,
          deviceName: deviceName,
        );
    state = AsyncData(result.session);
    ref.invalidate(syncServerUrlProvider);
    return result;
  }

  Future<AccountAuthResultDto> login({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    final result = await ref
        .read(accountBridgeProvider)
        .accountLogin(
          email: email,
          password: password,
          serverUrl: serverUrl,
          deviceName: deviceName,
        );
    state = AsyncData(result.session);
    ref.invalidate(syncServerUrlProvider);
    return result;
  }

  Future<void> logout() async {
    final bridge = ref.read(accountBridgeProvider);
    await bridge.accountLogout();
    await ref.read(billingStoreProvider).accountLoggedOut();
    state = AsyncData(await bridge.getAccountSessionState());
  }
}

final accountProvider =
    AsyncNotifierProvider<AccountNotifier, AccountSessionStateDto>(
      AccountNotifier.new,
    );

class BillingUiState {
  const BillingUiState({
    required this.entitlement,
    required this.products,
    this.busy = false,
    this.lastOutcome,
  });

  final BillingStateDto entitlement;
  final List<BillingProduct> products;
  final bool busy;
  final BillingPurchaseOutcome? lastOutcome;

  BillingUiState copyWith({
    BillingStateDto? entitlement,
    List<BillingProduct>? products,
    bool? busy,
    BillingPurchaseOutcome? lastOutcome,
  }) => BillingUiState(
    entitlement: entitlement ?? this.entitlement,
    products: products ?? this.products,
    busy: busy ?? this.busy,
    lastOutcome: lastOutcome ?? this.lastOutcome,
  );
}

class BillingNotifier extends AsyncNotifier<BillingUiState?> {
  @override
  Future<BillingUiState?> build() async {
    final account = await ref.watch(accountProvider.future);
    if (!account.loggedIn) return null;
    final bridge = ref.watch(billingBridgeProvider);
    final cached = await bridge.getCachedBilling();
    late final BillingStateDto entitlement;
    try {
      entitlement = await bridge.billingBootstrap();
    } catch (_) {
      if (cached == null) rethrow;
      return BillingUiState(entitlement: cached, products: const []);
    }
    return _withStoreCatalog(entitlement);
  }

  Future<BillingUiState> _withStoreCatalog(BillingStateDto entitlement) async {
    final store = ref.read(billingStoreProvider);
    try {
      await store.configure(
        appUserId: entitlement.providerAppUserId,
        environment: entitlement.environment,
      );
      return BillingUiState(
        entitlement: entitlement,
        products: await store.products(),
      );
    } catch (_) {
      return BillingUiState(entitlement: entitlement, products: const []);
    }
  }

  Future<void> refreshFromServer() async {
    final current = state.value;
    if (current == null) return;
    state = AsyncData(current.copyWith(busy: true));
    try {
      final entitlement = await ref
          .read(billingBridgeProvider)
          .refreshBilling();
      state = AsyncData(await _withStoreCatalog(entitlement));
    } catch (error, stackTrace) {
      state = AsyncError(error, stackTrace);
    }
  }

  Future<void> purchase(String productIdentifier) async {
    await _runStoreAction(
      () => ref.read(billingStoreProvider).purchase(productIdentifier),
    );
  }

  Future<void> restore() async {
    await _runStoreAction(() => ref.read(billingStoreProvider).restore());
  }

  Future<void> _runStoreAction(
    Future<BillingPurchaseOutcome> Function() action,
  ) async {
    final current = state.value;
    if (current == null || current.busy) return;
    state = AsyncData(current.copyWith(busy: true));
    try {
      final outcome = await action();
      if (outcome == BillingPurchaseOutcome.purchased) {
        final entitlement = await ref
            .read(billingBridgeProvider)
            .refreshBilling();
        final refreshed = await _withStoreCatalog(entitlement);
        state = AsyncData(refreshed.copyWith(lastOutcome: outcome));
      } else {
        state = AsyncData(current.copyWith(busy: false, lastOutcome: outcome));
      }
    } catch (_) {
      state = AsyncData(
        current.copyWith(
          busy: false,
          lastOutcome: BillingPurchaseOutcome.failed,
        ),
      );
    }
  }
}

final billingProvider = AsyncNotifierProvider<BillingNotifier, BillingUiState?>(
  BillingNotifier.new,
);

class SyncStatusNotifier extends AsyncNotifier<SyncStatusDto> {
  RealtimeSyncScheduler? _scheduler;
  bool _foreground = true;
  bool _connected = false;

  @override
  FutureOr<SyncStatusDto> build() async {
    _scheduler?.dispose();
    _foreground = ref.read(appForegroundProvider);
    final account = await ref.watch(accountProvider.future);
    final bridge = ref.watch(syncBridgeProvider);
    final status = await bridge.getSyncStatus();
    if (account.loggedIn) {
      // Bootstrap the server snapshot and provider identity at startup/login,
      // even when the Account screen has not been opened yet.
      unawaited(ref.read(billingProvider.future).catchError((_) => null));
    }
    final scheduler = RealtimeSyncScheduler(
      runSync: _performSync,
      timerFactory: ref.watch(realtimeTimerFactoryProvider),
      observer: ref.watch(realtimeEventSinkProvider),
    );
    _scheduler = scheduler;
    scheduler.setForeground(_foreground);
    scheduler.setConnected(_connected);
    ref.onDispose(scheduler.dispose);
    scheduleMicrotask(() {
      if (identical(_scheduler, scheduler)) {
        scheduler.setEnabled(account.loggedIn && status.loggedIn);
      }
    });
    return status;
  }

  Future<void> syncNow() {
    if (state.value?.loggedIn != true) {
      return _performSync();
    }
    return _scheduler?.syncNow() ?? _performSync();
  }

  void triggerRealtimeSync([
    RealtimeTriggerKind kind = RealtimeTriggerKind.localMutation,
  ]) => _scheduler?.trigger(kind);

  void setRealtimeConnected(bool connected) {
    _connected = connected;
    _scheduler?.setConnected(connected);
  }

  void setForeground(bool foreground) {
    _foreground = foreground;
    _scheduler?.setForeground(foreground);
  }

  Future<void> _performSync() async {
    final current = state.value;
    if (current != null) {
      state = AsyncData(_copySyncStatus(current, running: true));
    }
    try {
      final outcome = await ref.read(syncBridgeProvider).syncNowOutcome();
      switch (outcome) {
        case SyncNowOutcomeDto_Synced(:final status):
          state = AsyncData(status);
        case SyncNowOutcomeDto_BillingRequired():
          final recovered = await ref.read(syncBridgeProvider).getSyncStatus();
          state = AsyncData(_copySyncStatus(recovered, running: false));
          ref.invalidate(billingProvider);
          return;
      }
      ref.invalidate(listsProvider);
      ref.invalidate(archivedListsProvider);
      ref.invalidate(tasksProvider);
      ref.invalidate(homeTasksProvider);
      ref.invalidate(calendarOccurrencesProvider);
      ref.invalidate(latestTaskUndoProvider);
      ref.invalidate(taskRemindersProvider);
      ref.invalidate(completedTimerSessionsProvider);
      ref.invalidate(timerEngineProvider);
      ref.read(taskSearchProvider.notifier).refresh();
    } catch (_) {
      final failed = state.value;
      if (failed != null) {
        state = AsyncData(_copySyncStatus(failed, running: false));
      }
      try {
        final recovered = await ref.read(syncBridgeProvider).getSyncStatus();
        state = AsyncData(_copySyncStatus(recovered, running: false));
      } catch (_) {
        // Preserve the non-running snapshot above when status recovery also
        // fails. Sync failures are represented by SyncStatus, never by an
        // unhandled Future from automatic polling or a button callback.
      }
    }
  }

  Future<void> syncOnResume() async {
    final status = state.value;
    setForeground(true);
    if (status == null || !status.loggedIn) {
      return;
    }
    await ref.read(billingProvider.notifier).refreshFromServer();
    await syncNow();
  }
}

final syncStatusProvider =
    AsyncNotifierProvider<SyncStatusNotifier, SyncStatusDto>(
      SyncStatusNotifier.new,
    );

class AppForegroundNotifier extends Notifier<bool> {
  @override
  bool build() => true;

  void setForeground(bool foreground) {
    state = foreground;
  }
}

final appForegroundProvider = NotifierProvider<AppForegroundNotifier, bool>(
  AppForegroundNotifier.new,
);

final realtimeConnectionControllerProvider =
    Provider<RealtimeConnectionController?>((ref) {
      final foreground = ref.watch(appForegroundProvider);
      final account = ref.watch(accountProvider).value;
      if (!foreground || account?.loggedIn != true) {
        return null;
      }

      final bridge = ref.watch(bridgeServiceProvider);
      final controller = RealtimeConnectionController(
        fetchTicket: () async {
          final ticket = await bridge.getRealtimeTicket();
          return RealtimeTicketView(
            websocketUrl: ticket.websocketUrl,
            ticket: ticket.ticket,
            expiresAt: ticket.expiresAt,
          );
        },
        connector: ref.watch(realtimeSocketConnectorProvider),
        timerFactory: ref.watch(realtimeTimerFactoryProvider),
        observer: ref.watch(realtimeEventSinkProvider),
        onChanged: () {
          ref
              .read(syncStatusProvider.notifier)
              .triggerRealtimeSync(RealtimeTriggerKind.remoteHint);
        },
        onConnectionChanged: (connected) {
          ref.read(syncStatusProvider.notifier).setRealtimeConnected(connected);
        },
      );
      scheduleMicrotask(() => unawaited(controller.start()));
      ref.onDispose(() => unawaited(controller.dispose()));
      return controller;
    });

final completedTimerSyncTriggerProvider = Provider<void Function()>((ref) {
  return () => ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
});

SyncStatusDto _copySyncStatus(SyncStatusDto status, {bool? running}) {
  return SyncStatusDto(
    loggedIn: status.loggedIn,
    running: running ?? status.running,
    lastSuccessAt: status.lastSuccessAt,
    lastFailureAt: status.lastFailureAt,
    lastError: status.lastError,
    pushedCount: status.pushedCount,
    pushAckedCount: status.pushAckedCount,
    pushSupersededCount: status.pushSupersededCount,
    pulledCount: status.pulledCount,
    appliedCount: status.appliedCount,
    deletedCount: status.deletedCount,
    decryptFailedCount: status.decryptFailedCount,
    repushCount: status.repushCount,
    missingKeyQuarantinedCount: status.missingKeyQuarantinedCount,
    corruptionQuarantinedCount: status.corruptionQuarantinedCount,
    resolvedQuarantineCount: status.resolvedQuarantineCount,
    upgradeRequired: status.upgradeRequired,
  );
}

final settingsRepositoryProvider = Provider<SettingsRepository>(
  (ref) => SettingsRepository(ref.watch(settingsBridgeProvider)),
);

final reminderNotificationGatewayProvider =
    Provider<ReminderNotificationGateway>(
      (ref) => FlutterLocalReminderNotificationGateway(),
    );

final reminderNotificationServiceProvider =
    Provider<ReminderNotificationService>(
      (ref) => ReminderNotificationService(
        bridge: ref.watch(bridgeServiceProvider),
        gateway: ref.watch(reminderNotificationGatewayProvider),
      ),
    );

final timerClockProvider = Provider<TimerClock>(
  (ref) => const SystemTimerClock(),
);

final timerNotificationGatewayProvider = Provider<TimerNotificationGateway>(
  (ref) => FlutterLocalTimerNotificationGateway(),
);

final timerNotificationServiceProvider = Provider<TimerNotificationService>(
  (ref) =>
      TimerNotificationService(ref.watch(timerNotificationGatewayProvider)),
);

final timerSettingsProvider =
    AsyncNotifierProvider<TimerSettingsNotifier, TimerSettings>(
      () => TimerSettingsNotifier(
        bridgeServiceProvider,
        timerNotificationServiceProvider,
      ),
    );

final completedTimerSessionsProvider =
    FutureProvider.family<List<CompletedTimerSessionDto>, String>(
      (ref, taskId) => ref
          .watch(bridgeServiceProvider)
          .getCompletedTimerSessions(taskId: taskId),
    );

final timerEngineProvider =
    AsyncNotifierProvider<TimerEngineController, TimerEngineState>(
      () => TimerEngineController(
        bridgeServiceProvider,
        timerNotificationServiceProvider,
        timerClockProvider,
        (taskId) => completedTimerSessionsProvider(taskId),
        completedTimerSyncTriggerProvider,
      ),
    );

class TaskCompletionTimerSaveException implements Exception {
  const TaskCompletionTimerSaveException();
}

/// Serializes the one cross-feature completion invariant shared by every UI:
/// matching Focus work must be durably saved before the task becomes done.
class TaskCompletionCoordinator {
  TaskCompletionCoordinator(this._ref);

  final Ref _ref;

  Future<T> complete<T>({
    required String taskId,
    required Future<T> Function() setDone,
  }) async {
    final engine = await _ref.read(timerEngineProvider.future);
    final active = engine.active;
    final matchingWork =
        active?.taskId == taskId && active?.phase == TimerPhaseDto.work;
    final activeBreak = active != null && active.phase != TimerPhaseDto.work;
    if (matchingWork || activeBreak) {
      final activeSession = active!;
      final completed = await _ref
          .read(timerEngineProvider.notifier)
          .finish(kind: TimerFinishKindDto.completed);
      final remaining = _ref.read(timerEngineProvider).value?.active;
      final wasCleared = remaining?.sessionId != activeSession.sessionId;
      final workWasSaved = !matchingWork || completed != null;
      if (!wasCleared || !workWasSaved) {
        throw const TaskCompletionTimerSaveException();
      }
    }
    return setDone();
  }
}

final taskCompletionCoordinatorProvider = Provider<TaskCompletionCoordinator>(
  (ref) => TaskCompletionCoordinator(ref),
);

/// Provides the reserved F-01 UI mode setting.
///
/// Phase 1 exposes only the persistence port. Selection/onboarding UI is a
/// Phase 3 concern.
class UiModeNotifier extends AsyncNotifier<String> {
  @override
  FutureOr<String> build() {
    return ref.watch(settingsRepositoryProvider).getUiMode();
  }

  Future<void> setUiMode(String uiMode) async {
    await ref.read(settingsRepositoryProvider).setUiMode(uiMode);
    ref.invalidateSelf();
  }
}

final uiModeProvider = AsyncNotifierProvider<UiModeNotifier, String>(
  UiModeNotifier.new,
);

class CalendarWeekStartNotifier extends AsyncNotifier<String> {
  @override
  FutureOr<String> build() {
    return ref.watch(settingsRepositoryProvider).getCalendarWeekStart();
  }

  Future<void> setWeekStart(String weekStart) async {
    final previous = state.value ?? defaultCalendarWeekStart;
    try {
      await ref
          .read(settingsRepositoryProvider)
          .setCalendarWeekStart(weekStart);
      state = AsyncData(weekStart);
    } catch (error, stackTrace) {
      state = AsyncData(previous);
      Error.throwWithStackTrace(error, stackTrace);
    }
  }
}

final calendarWeekStartProvider =
    AsyncNotifierProvider<CalendarWeekStartNotifier, String>(
      CalendarWeekStartNotifier.new,
    );

/// Gates the one-time welcome experience before the app starts its ordinary
/// Home and sync providers. The flag is device-local and remains inside the
/// encrypted settings table; it is intentionally not synchronized.
class OnboardingStatusNotifier extends AsyncNotifier<bool> {
  @override
  FutureOr<bool> build() async {
    final value = await ref
        .watch(settingsRepositoryProvider)
        .getSetting(onboardingCompletedSettingKey);
    return value == '1';
  }

  Future<void> complete() async {
    await ref
        .read(settingsRepositoryProvider)
        .setSetting(onboardingCompletedSettingKey, '1');
    state = const AsyncData(true);
  }
}

final onboardingStatusProvider =
    AsyncNotifierProvider<OnboardingStatusNotifier, bool>(
      OnboardingStatusNotifier.new,
    );

/// Generates a placeholder, monotonically-appending sort order string (e.g.
/// `a0`, `a1`, `a2`, ...) for newly created lists in this UI skeleton.
///
/// This is intentionally NOT a real fractional-index implementation: it
/// cannot express "insert between two existing items" or rebalance existing
/// values. Task sort orders are generated by the Rust/domain layer.
String nextSortOrder(int existingItemCount) => 'a$existingItemCount';

/// Manages the list of [ListDto]s shown on the lists screen.
///
/// Invalidate strategy: [createList] performs the bridge call first, then
/// calls `ref.invalidateSelf()`, which re-runs [build] and re-fetches
/// `getLists()`. Any widget that does `ref.watch(listsProvider)` is rebuilt
/// automatically with the refreshed `AsyncValue`.
class ListsNotifier extends AsyncNotifier<List<ListDto>> {
  @override
  FutureOr<List<ListDto>> build() {
    return ref.watch(bridgeServiceProvider).getLists();
  }

  /// Creates a new list named `name` and refreshes [listsProvider].
  Future<void> createList(String name) async {
    final bridge = ref.read(bridgeServiceProvider);
    final sortOrder = nextSortOrder(state.value?.length ?? 0);
    await bridge.createList(name: name, sortOrder: sortOrder);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  /// Renames `listId` and refreshes [listsProvider].
  Future<void> renameList(String listId, String name) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.renameList(listId: listId, name: name);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
    ref.invalidate(archivedListsProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidate(homeTasksProvider);
    ref.read(taskSearchProvider.notifier).refresh();
  }

  /// Archives `listId` and refreshes active and archived list collections.
  Future<void> archiveList(String listId) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.archiveList(listId: listId);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
    ref.invalidate(archivedListsProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidate(homeTasksProvider);
    ref.read(taskSearchProvider.notifier).refresh();
  }

  Future<int> countTasks(String listId) {
    return ref.read(bridgeServiceProvider).countTasksInList(listId: listId);
  }

  /// Deletes `listId`, rehomes its tasks, and refreshes list collections.
  Future<void> deleteList(String listId) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.deleteList(listId: listId);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
    ref.invalidate(archivedListsProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidate(completedTimerSessionsProvider);
    ref.invalidate(homeTasksProvider);
    ref.invalidate(timerEngineProvider);
    ref.read(taskSearchProvider.notifier).refresh();
  }
}

final listsProvider = AsyncNotifierProvider<ListsNotifier, List<ListDto>>(
  ListsNotifier.new,
);

/// Manages archived lists shown in the collapsed archive section.
class ArchivedListsNotifier extends AsyncNotifier<List<ListDto>> {
  @override
  FutureOr<List<ListDto>> build() {
    return ref.watch(bridgeServiceProvider).getArchivedLists();
  }

  /// Restores `listId` and refreshes archived and active list collections.
  Future<void> unarchiveList(String listId) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.unarchiveList(listId: listId);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
    ref.invalidate(listsProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidate(homeTasksProvider);
    ref.read(taskSearchProvider.notifier).refresh();
  }
}

final archivedListsProvider =
    AsyncNotifierProvider<ArchivedListsNotifier, List<ListDto>>(
      ArchivedListsNotifier.new,
    );

/// Keeps the selected task display order in memory for the current app
/// session. Phase 1 intentionally does not persist this value to storage.
class TaskSortModeNotifier extends Notifier<TaskSortMode> {
  TaskSortModeNotifier(this.listId);

  final String listId;

  @override
  TaskSortMode build() {
    return TaskSortMode.manual;
  }

  void setMode(TaskSortMode mode) {
    state = mode;
  }
}

final taskSortModeProvider =
    NotifierProvider.family<TaskSortModeNotifier, TaskSortMode, String>(
      TaskSortModeNotifier.new,
    );

({int startMs, int endMs}) todayLocalRangeMs({DateTime? now}) {
  final current = now ?? DateTime.now();
  final start = localCivilDay(current);
  final end = localCivilDay(current, dayOffset: 1);
  return (
    startMs: start.millisecondsSinceEpoch,
    endMs: end.millisecondsSinceEpoch,
  );
}

({int todayStartMs, int tomorrowStartMs, int dayAfterTomorrowStartMs})
homeLocalRangesMs({DateTime? now}) {
  final current = now ?? DateTime.now();
  final todayStart = localCivilDay(current);
  final tomorrowStart = localCivilDay(current, dayOffset: 1);
  final dayAfterTomorrowStart = localCivilDay(current, dayOffset: 2);
  return (
    todayStartMs: todayStart.millisecondsSinceEpoch,
    tomorrowStartMs: tomorrowStart.millisecondsSinceEpoch,
    dayAfterTomorrowStartMs: dayAfterTomorrowStart.millisecondsSinceEpoch,
  );
}

/// Value identity for a Calendar query. Civil dates select date-only due
/// occurrences while UTC instants select datetime/scheduled/completed ones.
class CalendarRange {
  const CalendarRange._({
    required this.startOn,
    required this.endOn,
    required this.startAt,
    required this.endAt,
  });

  factory CalendarRange.local({
    required DateTime start,
    required DateTime end,
  }) {
    if (start.isUtc || end.isUtc) {
      throw ArgumentError('calendar range boundaries must be viewer-local');
    }
    if (start.hour != 0 ||
        start.minute != 0 ||
        start.second != 0 ||
        start.millisecond != 0 ||
        start.microsecond != 0 ||
        end.hour != 0 ||
        end.minute != 0 ||
        end.second != 0 ||
        end.millisecond != 0 ||
        end.microsecond != 0) {
      throw ArgumentError('calendar range boundaries must be local midnight');
    }
    if (!end.isAfter(start)) {
      throw ArgumentError('calendar range must be non-empty');
    }
    final startOn = _civilDateFromFields(start);
    final endOn = _civilDateFromFields(end);
    if (startOn.compareTo(endOn) >= 0) {
      throw ArgumentError('calendar civil range must be increasing');
    }
    return CalendarRange._(
      startOn: startOn,
      endOn: endOn,
      startAt: start.toUtc(),
      endAt: end.toUtc(),
    );
  }

  factory CalendarRange.day(DateTime day) {
    return CalendarRange.local(
      start: localCivilDay(day),
      end: localCivilDay(day, dayOffset: 1),
    );
  }

  final String startOn;
  final String endOn;
  final DateTime startAt;
  final DateTime endAt;

  CalendarRangeInput toInput() => CalendarRangeInput(
    startOn: startOn,
    endOn: endOn,
    startAt: startAt,
    endAt: endAt,
  );

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is CalendarRange &&
          startOn == other.startOn &&
          endOn == other.endOn &&
          startAt == other.startAt &&
          endAt == other.endAt;

  @override
  int get hashCode => Object.hash(startOn, endOn, startAt, endAt);
}

String _civilDateFromFields(DateTime value) =>
    '${value.year.toString().padLeft(4, '0')}-'
    '${value.month.toString().padLeft(2, '0')}-'
    '${value.day.toString().padLeft(2, '0')}';

class CalendarOccurrencesNotifier
    extends AsyncNotifier<List<CalendarOccurrenceDto>> {
  CalendarOccurrencesNotifier(this.range);

  final CalendarRange range;

  @override
  FutureOr<List<CalendarOccurrenceDto>> build() {
    return ref
        .watch(bridgeServiceProvider)
        .getCalendarOccurrences(range: range.toInput());
  }

  Future<void> moveOccurrence({
    required CalendarOccurrenceDto occurrence,
    required DateTime targetDate,
  }) async {
    final task = occurrence.task;
    var due = task.due;
    var scheduledAt = task.scheduledAt;
    final target = targetDate.toLocal();

    switch (occurrence.kind) {
      case CalendarOccurrenceKindDto_DateDue():
        due = dateOnlyDue(target);
      case CalendarOccurrenceKindDto_DateTimeDue(:final dueAt, :final timeZone):
        final savedWallClock = taskDueDisplayDate(
          TaskDueDto.dateTime(dueAt: dueAt, timeZone: timeZone),
        );
        due = dateTimeDue(
          localDateTime: DateTime(
            target.year,
            target.month,
            target.day,
            savedWallClock.hour,
            savedWallClock.minute,
            savedWallClock.second,
            savedWallClock.millisecond,
            savedWallClock.microsecond,
          ),
          timeZone: timeZone,
        );
      case CalendarOccurrenceKindDto_Scheduled(
        scheduledAt: final savedScheduledAt,
      ):
        final savedWallClock = savedScheduledAt.toLocal();
        scheduledAt = DateTime(
          target.year,
          target.month,
          target.day,
          savedWallClock.hour,
          savedWallClock.minute,
          savedWallClock.second,
          savedWallClock.millisecond,
        ).millisecondsSinceEpoch;
      case CalendarOccurrenceKindDto_Completed():
        throw StateError('completed occurrences cannot be moved');
    }

    final updated = await ref
        .read(bridgeServiceProvider)
        .updateTask(
          taskId: task.id,
          title: task.title,
          note: task.note,
          priority: task.priority,
          due: due == null ? null : taskDueInput(due),
          scheduledAt: scheduledAt,
          estimatedMinutes: task.estimatedMinutes,
        );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidate(tasksProvider(updated.listId));
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.read(taskSearchProvider.notifier).refresh();
  }
}

final calendarOccurrencesProvider =
    AsyncNotifierProvider.family<
      CalendarOccurrencesNotifier,
      List<CalendarOccurrenceDto>,
      CalendarRange
    >(CalendarOccurrencesNotifier.new);

/// Manages the tasks of a single list, keyed by `listId`.
///
/// Invalidate strategy: [createTask], [updateTask], [setStatus] and [deleteTask] each
/// perform their bridge call first, then call `ref.invalidateSelf()`, which
/// re-runs [build] for this `listId` only (other lists' [TasksNotifier]
/// instances are untouched). [taskDetailProvider] derives its value from
/// this provider via `ref.watch`, so it is refreshed transitively whenever
/// this provider is invalidated -- no separate invalidate call is needed for
/// the detail screen.
class TasksNotifier extends AsyncNotifier<List<TaskDto>> {
  TasksNotifier(this.listId);

  final String listId;

  @override
  FutureOr<List<TaskDto>> build() {
    return ref.watch(bridgeServiceProvider).getTasks(listId: listId);
  }

  /// Creates a new task titled `title` in this list and refreshes the task
  /// list. When [parentTaskId] is provided, the new task is created as a
  /// subtask of that parent. The Rust/domain layer assigns the task sort order
  /// within the target sibling group.
  Future<void> createTask(
    String title, {
    String? parentTaskId,
    TaskDueDto? due,
    String note = '',
    int priority = 0,
    int? scheduledAt,
    int? estimatedMinutes,
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.createTask(
      listId: listId,
      title: title,
      parentTaskId: parentTaskId,
      due: due == null ? null : taskDueInput(due),
      note: note,
      priority: priority,
      scheduledAt: scheduledAt,
      estimatedMinutes: estimatedMinutes,
    );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  /// Updates editable task fields and refreshes the task list.
  Future<void> updateTask({
    required String taskId,
    required String title,
    required String note,
    required int priority,
    required TaskDueDto? due,
    required int? scheduledAt,
    required int? estimatedMinutes,
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.updateTask(
      taskId: taskId,
      title: title,
      note: note,
      priority: priority,
      due: due == null ? null : taskDueInput(due),
      scheduledAt: scheduledAt,
      estimatedMinutes: estimatedMinutes,
    );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidate(latestTaskUndoProvider);
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  Future<void> updateDue(TaskDto task, TaskDueDto? due) async {
    await updateTask(
      taskId: task.id,
      title: task.title,
      note: task.note,
      priority: task.priority,
      due: due,
      scheduledAt: task.scheduledAt,
      estimatedMinutes: task.estimatedMinutes,
    );
    ref.invalidate(homeTasksProvider);
  }

  /// Transitions `taskId` to `status` and refreshes the task list.
  Future<void> setStatus(
    String taskId,
    String status, {
    String? closedReason,
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    final reminders = status == 'done' || status == 'wont_do'
        ? await bridge.getTaskReminders(taskId: taskId)
        : const <ReminderDto>[];
    Future<void> persistStatus() async {
      await bridge.setTaskStatus(
        taskId: taskId,
        status: status,
        closedReason: closedReason,
      );
    }

    if (status == 'done') {
      await ref
          .read(taskCompletionCoordinatorProvider)
          .complete(taskId: taskId, setDone: persistStatus);
    } else {
      await persistStatus();
    }
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    if (status == 'done' || status == 'wont_do') {
      await ref
          .read(reminderNotificationServiceProvider)
          .cancelReminders(reminders);
      ref.invalidate(latestTaskUndoProvider);
      ref.invalidate(taskRemindersProvider(taskId));
    } else {
      await ref
          .read(reminderNotificationServiceProvider)
          .scheduleTaskReminders(taskId);
    }
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  Future<int> countDescendants(String taskId) {
    return ref.read(bridgeServiceProvider).countTaskDescendants(taskId: taskId);
  }

  /// Permanently deletes `taskId` and its descendants, then refreshes the list.
  Future<void> deleteTask(String taskId) async {
    final bridge = ref.read(bridgeServiceProvider);
    final reminders = await bridge.getTaskSubtreeReminders(taskId: taskId);
    await bridge.deleteTask(taskId: taskId);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    await ref
        .read(reminderNotificationServiceProvider)
        .cancelReminders(reminders);
    ref.invalidate(taskRemindersProvider(taskId));
    ref.invalidate(completedTimerSessionsProvider(taskId));
    ref.invalidate(timerEngineProvider);
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  /// Moves `taskId` between sibling boundaries and refreshes the task list.
  Future<void> reorderTask({
    required String taskId,
    required String? previousTaskId,
    required String? nextTaskId,
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.reorderTask(
      taskId: taskId,
      previousTaskId: previousTaskId,
      nextTaskId: nextTaskId,
    );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidateSelf();
  }
}

final tasksProvider =
    AsyncNotifierProvider.family<TasksNotifier, List<TaskDto>, String>(
      TasksNotifier.new,
    );

/// Manages the cross-list Home smart view.
class HomeTasksNotifier extends AsyncNotifier<List<HomeTaskDto>> {
  @override
  FutureOr<List<HomeTaskDto>> build() {
    final range = homeLocalRangesMs();
    return ref
        .watch(bridgeServiceProvider)
        .getHomeTasks(
          todayStartMs: range.todayStartMs,
          tomorrowStartMs: range.tomorrowStartMs,
        );
  }

  Future<void> createTask({
    required String listId,
    required String title,
    required TaskDueDto? due,
    required int priority,
    required int? scheduledAt,
    required int? estimatedMinutes,
    String note = '',
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    await bridge.createTask(
      listId: listId,
      title: title,
      due: due == null ? null : taskDueInput(due),
      note: note,
      priority: priority,
      scheduledAt: scheduledAt,
      estimatedMinutes: estimatedMinutes,
    );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidate(tasksProvider(listId));
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  Future<void> setStatus(
    String taskId,
    String status, {
    String? closedReason,
  }) async {
    final bridge = ref.read(bridgeServiceProvider);
    final reminders = status == 'done' || status == 'wont_do'
        ? await bridge.getTaskReminders(taskId: taskId)
        : const <ReminderDto>[];
    Future<TaskDto> persistStatus() => bridge.setTaskStatus(
      taskId: taskId,
      status: status,
      closedReason: closedReason,
    );
    final updated = status == 'done'
        ? await ref
              .read(taskCompletionCoordinatorProvider)
              .complete(taskId: taskId, setDone: persistStatus)
        : await persistStatus();
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    if (status == 'done' || status == 'wont_do') {
      await ref
          .read(reminderNotificationServiceProvider)
          .cancelReminders(reminders);
      ref.invalidate(latestTaskUndoProvider);
      ref.invalidate(taskRemindersProvider(taskId));
    } else {
      await ref
          .read(reminderNotificationServiceProvider)
          .scheduleTaskReminders(taskId);
    }
    ref.invalidate(tasksProvider(updated.listId));
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }

  Future<void> updateDue(TaskDto task, TaskDueDto? due) async {
    final bridge = ref.read(bridgeServiceProvider);
    final updated = await bridge.updateTask(
      taskId: task.id,
      title: task.title,
      note: task.note,
      priority: task.priority,
      due: due == null ? null : taskDueInput(due),
      scheduledAt: task.scheduledAt,
      estimatedMinutes: task.estimatedMinutes,
    );
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    ref.invalidate(latestTaskUndoProvider);
    ref.invalidate(tasksProvider(updated.listId));
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
  }
}

final homeTasksProvider =
    AsyncNotifierProvider<HomeTasksNotifier, List<HomeTaskDto>>(
      HomeTasksNotifier.new,
    );

/// Identifies a single task for [taskDetailProvider]: the containing list id
/// plus the task id.
typedef TaskDetailArgs = ({String listId, String taskId});

/// Task detail lookup policy (M2-03): there is no dedicated "get task by
/// id" bridge API exposed yet, so the detail screen derives its data by
/// watching [tasksProvider] for the task's list and finding the matching
/// task client-side. This keeps a single cache/source of truth for tasks
/// (avoids a second, possibly stale, copy of task data) and avoids an extra
/// round trip to the bridge. If a dedicated get-task-by-id bridge call is
/// added later, this provider's body can be swapped to call it directly
/// without changing the screen that consumes it.
final taskDetailProvider =
    Provider.family<AsyncValue<TaskDto?>, TaskDetailArgs>((ref, args) {
      final tasksAsync = ref.watch(tasksProvider(args.listId));
      return tasksAsync.whenData((tasks) {
        for (final task in tasks) {
          if (task.id == args.taskId) {
            return task;
          }
        }
        return null;
      });
    });

/// Manages the latest task undo entry and applies undo through the bridge.
class LatestTaskUndoNotifier extends AsyncNotifier<TaskUndoDto?> {
  @override
  FutureOr<TaskUndoDto?> build() {
    return ref.watch(bridgeServiceProvider).getLatestTaskUndo();
  }

  Future<TaskDto> undo(String undoId) async {
    final restored = await ref
        .read(bridgeServiceProvider)
        .undoTaskOperation(undoId: undoId);
    ref.read(syncStatusProvider.notifier).triggerRealtimeSync();
    await ref
        .read(reminderNotificationServiceProvider)
        .scheduleTaskReminders(restored.id);
    ref.invalidate(tasksProvider(restored.listId));
    ref.invalidate(homeTasksProvider);
    ref.invalidate(calendarOccurrencesProvider);
    ref.invalidateSelf();
    ref.read(taskSearchProvider.notifier).refresh();
    await ref.read(tasksProvider(restored.listId).future);
    return restored;
  }
}

final latestTaskUndoProvider =
    AsyncNotifierProvider<LatestTaskUndoNotifier, TaskUndoDto?>(
      LatestTaskUndoNotifier.new,
    );

/// Manages reminders attached to a single task.
class TaskRemindersNotifier extends AsyncNotifier<List<ReminderDto>> {
  TaskRemindersNotifier(this.taskId);

  final String taskId;

  @override
  FutureOr<List<ReminderDto>> build() {
    return ref.watch(reminderBridgeProvider).getTaskReminders(taskId: taskId);
  }

  Future<ReminderDto> createReminder(int remindAt) async {
    final reminder = await ref
        .read(reminderBridgeProvider)
        .createTaskReminder(taskId: taskId, remindAt: remindAt);
    ref.invalidateSelf();
    return reminder;
  }

  Future<ReminderDto> updateReminder(String reminderId, int remindAt) async {
    final reminder = await ref
        .read(reminderBridgeProvider)
        .updateReminder(reminderId: reminderId, remindAt: remindAt);
    ref.invalidateSelf();
    return reminder;
  }

  Future<ReminderDto> deleteReminder(String reminderId) async {
    final reminder = await ref
        .read(reminderBridgeProvider)
        .deleteReminder(reminderId: reminderId);
    await ref
        .read(reminderNotificationServiceProvider)
        .cancelReminder(reminder);
    ref.invalidateSelf();
    return reminder;
  }

  Future<List<ReminderDto>> clearReminders() async {
    final reminders = await ref
        .read(reminderBridgeProvider)
        .clearTaskReminders(taskId: taskId);
    await ref
        .read(reminderNotificationServiceProvider)
        .cancelReminders(reminders);
    ref.invalidateSelf();
    return reminders;
  }
}

final taskRemindersProvider =
    AsyncNotifierProvider.family<
      TaskRemindersNotifier,
      List<ReminderDto>,
      String
    >(TaskRemindersNotifier.new);

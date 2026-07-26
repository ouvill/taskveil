import 'dart:convert';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/timer/timer_engine.dart';
import 'package:taskveil/src/timer/timer_notifications.dart';
import 'package:taskveil/src/timer/timer_settings.dart';

import 'support/fake_bridge_service.dart';

void main() {
  test('settings default to 25/5/15/4 and reject invalid increments', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);

    expect(
      await harness.container.read(timerSettingsProvider.future),
      const TimerSettings(),
    );
    expect(
      () => const TimerSettings(workMinutes: 26).validated(),
      throwsA(isA<TimerSettingsValidationException>()),
    );
  });

  test('denied notification opt-in is persisted disabled', () async {
    final gateway = _FakeTimerNotificationGateway(permissionsGranted: false);
    final harness = await _Harness.create(gateway: gateway);
    addTearDown(harness.dispose);

    await harness.container.read(timerSettingsProvider.future);
    await harness.container
        .read(timerSettingsProvider.notifier)
        .save(const TimerSettings(notificationsEnabled: true));

    expect(gateway.permissionRequests, 1);
    expect(
      harness.container
          .read(timerSettingsProvider)
          .requireValue
          .notificationsEnabled,
      isFalse,
    );
    final persisted = await harness.bridge.getFrontendSetting(
      key: timerSettingsKey,
    );
    expect(jsonDecode(persisted!)['notificationsEnabled'], isFalse);
  });

  test(
    'Stopwatch pause and resume exclude paused wall time and preserve task status',
    () async {
      final harness = await _Harness.create();
      addTearDown(harness.dispose);
      final task = harness.task;

      await harness.container
          .read(timerEngineProvider.notifier)
          .startStopwatch(taskId: task.id);
      harness.clock.advance(const Duration(minutes: 3));
      await harness.container.read(timerEngineProvider.notifier).pause();
      harness.clock.advance(const Duration(hours: 2));
      await harness.container.read(timerEngineProvider.notifier).resume();
      harness.clock.advance(const Duration(minutes: 2));
      final completed = await harness.container
          .read(timerEngineProvider.notifier)
          .finish();

      expect(
        completed!.activeDurationMs,
        const Duration(minutes: 5).inMilliseconds,
      );
      expect(
        (await harness.bridge.getTasks(listId: task.listId)).single.status,
        'todo',
      );
      expect(await harness.bridge.getActiveTimerSession(), isNull);
    },
  );

  test('resume settles Pomodoro at the Rust target instant', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);

    await harness.container
        .read(timerEngineProvider.notifier)
        .startPomodoro(taskId: harness.task.id);
    final startedAt = harness.clock.now();
    harness.clock.advance(const Duration(hours: 1));
    await harness.container.read(timerEngineProvider.notifier).settleOnResume();

    final completed = (await harness.bridge.getCompletedTimerSessions(
      taskId: harness.task.id,
    )).single;
    expect(completed.endedAt, startedAt.add(const Duration(minutes: 25)));
    expect(
      completed.activeDurationMs,
      const Duration(minutes: 25).inMilliseconds,
    );
    expect(completed.finishKind, TimerFinishKindDto.completed);
  });

  test(
    'foreground display tick settles Pomodoro at the target instant',
    () async {
      final harness = await _Harness.create();
      addTearDown(harness.dispose);
      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      final startedAt = harness.clock.now();

      harness.clock.advance(const Duration(minutes: 25));
      await harness.container
          .read(timerEngineProvider.notifier)
          .refreshDisplay();

      final state = harness.container.read(timerEngineProvider).requireValue;
      expect(state.active, isNull);
      expect(state.lastCompletion, isNotNull);
      expect(state.isBreakPending, isTrue);
      final completed = (await harness.bridge.getCompletedTimerSessions(
        taskId: harness.task.id,
      )).single;
      expect(completed.endedAt, startedAt.add(const Duration(minutes: 25)));
      expect(
        completed.activeDurationMs,
        const Duration(minutes: 25).inMilliseconds,
      );
      expect(completed.finishKind, TimerFinishKindDto.completed);
    },
  );

  test(
    'foreground display tick finishes a break without saving work',
    () async {
      final harness = await _Harness.create();
      addTearDown(harness.dispose);
      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      harness.clock.advance(const Duration(minutes: 1));
      await harness.container.read(timerEngineProvider.notifier).finish();
      await harness.container.read(timerEngineProvider.notifier).startBreak();

      harness.clock.advance(const Duration(minutes: 5));
      await harness.container
          .read(timerEngineProvider.notifier)
          .refreshDisplay();

      final state = harness.container.read(timerEngineProvider).requireValue;
      expect(state.active, isNull);
      expect(state.breakJustCompleted, isTrue);
      expect(state.isBreakPending, isFalse);
      expect(
        await harness.bridge.getCompletedTimerSessions(taskId: harness.task.id),
        hasLength(1),
      );
    },
  );

  test('foreground finish and pause settle at elapsed boundaries', () async {
    final finishHarness = await _Harness.create();
    addTearDown(finishHarness.dispose);
    await finishHarness.container
        .read(timerEngineProvider.notifier)
        .startPomodoro(taskId: finishHarness.task.id);
    final pomodoroStart = finishHarness.clock.now();
    finishHarness.clock.advance(const Duration(minutes: 40));
    final completed = await finishHarness.container
        .read(timerEngineProvider.notifier)
        .finish();
    expect(completed!.endedAt, pomodoroStart.add(const Duration(minutes: 25)));

    final pauseHarness = await _Harness.create();
    addTearDown(pauseHarness.dispose);
    await pauseHarness.container
        .read(timerEngineProvider.notifier)
        .startStopwatch(taskId: pauseHarness.task.id);
    final stopwatchStart = pauseHarness.clock.now();
    pauseHarness.clock.advance(const Duration(days: 8));
    await pauseHarness.container.read(timerEngineProvider.notifier).pause();
    final interrupted = (await pauseHarness.bridge.getCompletedTimerSessions(
      taskId: pauseHarness.task.id,
    )).single;
    expect(interrupted.endedAt, stopwatchStart.add(const Duration(days: 7)));
    expect(interrupted.finishKind, TimerFinishKindDto.interrupted);
  });

  test('explicit add time extends a reached display target', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);
    await harness.container
        .read(timerEngineProvider.notifier)
        .startPomodoro(taskId: harness.task.id);
    final startedAt = harness.clock.now();
    harness.clock.advance(const Duration(minutes: 26));
    await harness.container
        .read(timerEngineProvider.notifier)
        .addTime(const Duration(minutes: 5));
    expect(await harness.bridge.getActiveTimerSession(), isNotNull);
    harness.clock.advance(const Duration(minutes: 4));
    await harness.container.read(timerEngineProvider.notifier).settleOnResume();
    final completed = (await harness.bridge.getCompletedTimerSessions(
      taskId: harness.task.id,
    )).single;
    expect(completed.endedAt, startedAt.add(const Duration(minutes: 30)));
  });

  test('four completed Pomodoros durably select a long break', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);
    for (var index = 0; index < 4; index += 1) {
      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      harness.clock.advance(const Duration(minutes: 1));
      await harness.container.read(timerEngineProvider.notifier).finish();
    }
    expect(
      harness.container.read(timerEngineProvider).requireValue.nextBreakPhase,
      TimerPhaseDto.longBreak,
    );
    await harness.container.read(timerEngineProvider.notifier).startBreak();
    final active = await harness.bridge.getActiveTimerSession();
    expect(active!.phase, TimerPhaseDto.longBreak);
    expect(active.targetDurationMs, const Duration(minutes: 15).inMilliseconds);
  });

  test(
    'completed work restores a pending break and phase after restart',
    () async {
      final harness = await _Harness.create();
      addTearDown(harness.dispose);
      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      harness.clock.advance(const Duration(minutes: 1));
      await harness.container.read(timerEngineProvider.notifier).finish();

      final restored = await _Harness.create(
        bridge: harness.bridge,
        clock: harness.clock,
        createTask: false,
      );
      addTearDown(restored.dispose);
      final state = await restored.container.read(timerEngineProvider.future);
      expect(state.isBreakPending, isTrue);
      expect(state.nextBreakPhase, TimerPhaseDto.shortBreak);
    },
  );

  test(
    'break start journal consumes pending exactly once after crash',
    () async {
      final bridge = _FailingRuntimeBridge();
      final harness = await _Harness.create(bridge: bridge);
      addTearDown(harness.dispose);
      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      harness.clock.advance(const Duration(minutes: 1));
      await harness.container.read(timerEngineProvider.notifier).finish();
      bridge.failBreakCommit = true;

      await expectLater(
        harness.container.read(timerEngineProvider.notifier).startBreak(),
        throwsA(isA<StateError>()),
      );
      final restored = await _Harness.create(
        bridge: bridge,
        clock: harness.clock,
        createTask: false,
      );
      addTearDown(restored.dispose);
      final restoredState = await restored.container.read(
        timerEngineProvider.future,
      );
      expect(restoredState.active?.phase, TimerPhaseDto.shortBreak);
      expect(restoredState.isBreakPending, isFalse);

      await restored.container.read(timerEngineProvider.notifier).discard();
      final restarted = await _Harness.create(
        bridge: bridge,
        clock: harness.clock,
        createTask: false,
      );
      addTearDown(restarted.dispose);
      expect(
        (await restarted.container.read(
          timerEngineProvider.future,
        )).isBreakPending,
        isFalse,
      );
      await expectLater(
        restarted.container.read(timerEngineProvider.notifier).startBreak(),
        throwsA(isA<TimerEngineStateException>()),
      );
    },
  );

  test('failed break start keeps the break pending', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);
    await harness.container
        .read(timerEngineProvider.notifier)
        .startPomodoro(taskId: harness.task.id);
    harness.clock.advance(const Duration(minutes: 1));
    await harness.container.read(timerEngineProvider.notifier).finish();
    await harness.bridge.startActiveTimerSession(
      session: ActiveTimerSessionDto(
        sessionId: '00000000-0000-4000-8000-000000000077',
        taskId: harness.task.id,
        mode: TimerModeDto.stopwatch,
        phase: TimerPhaseDto.work,
        state: TimerRunStateDto.running,
        startedAt: harness.clock.now(),
        lastResumedAt: harness.clock.now(),
        accumulatedActiveMs: 0,
      ),
    );

    await expectLater(
      harness.container.read(timerEngineProvider.notifier).startBreak(),
      throwsA(isA<TimerActiveConflictException>()),
    );
    expect(
      harness.container.read(timerEngineProvider).requireValue.isBreakPending,
      isTrue,
    );
  });

  test('skip break acknowledgement survives restart', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);
    await harness.container
        .read(timerEngineProvider.notifier)
        .startPomodoro(taskId: harness.task.id);
    harness.clock.advance(const Duration(minutes: 1));
    await harness.container.read(timerEngineProvider.notifier).finish();
    expect(
      harness.container.read(timerEngineProvider).requireValue.isBreakPending,
      isTrue,
    );
    await harness.container.read(timerEngineProvider.notifier).skipBreak();

    final restored = await _Harness.create(
      bridge: harness.bridge,
      clock: harness.clock,
      createTask: false,
    );
    addTearDown(restored.dispose);
    expect(
      (await restored.container.read(
        timerEngineProvider.future,
      )).isBreakPending,
      isFalse,
    );
  });

  test('seven-day cap saves running work interrupted at the cap', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);

    await harness.container
        .read(timerEngineProvider.notifier)
        .startStopwatch(taskId: harness.task.id);
    final startedAt = harness.clock.now();
    harness.clock.advance(const Duration(days: 8));
    await harness.container.read(timerEngineProvider.notifier).settleOnResume();

    final completed = (await harness.bridge.getCompletedTimerSessions(
      taskId: harness.task.id,
    )).single;
    expect(completed.endedAt, startedAt.add(const Duration(days: 7)));
    expect(completed.activeDurationMs, const Duration(days: 7).inMilliseconds);
    expect(completed.finishKind, TimerFinishKindDto.interrupted);
  });

  test(
    'zero-duration manual finish and expired paused timer discard locally',
    () async {
      final harness = await _Harness.create();
      addTearDown(harness.dispose);

      await harness.container
          .read(timerEngineProvider.notifier)
          .startStopwatch(taskId: harness.task.id);
      expect(
        await harness.container.read(timerEngineProvider.notifier).finish(),
        isNull,
      );
      expect(await harness.bridge.getActiveTimerSession(), isNull);

      await harness.container
          .read(timerEngineProvider.notifier)
          .startStopwatch(taskId: harness.task.id);
      await harness.container.read(timerEngineProvider.notifier).pause();
      harness.clock.advance(const Duration(days: 8));
      await harness.container.read(timerEngineProvider.notifier).resume();
      expect(await harness.bridge.getActiveTimerSession(), isNull);
      expect(
        await harness.bridge.getCompletedTimerSessions(taskId: harness.task.id),
        isEmpty,
      );
    },
  );

  test(
    'pending cycle journal reconciles exactly once after commit-write crash',
    () async {
      final bridge = _FailingRuntimeBridge();
      final harness = await _Harness.create(bridge: bridge);
      addTearDown(harness.dispose);

      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      harness.clock.advance(const Duration(minutes: 1));
      bridge.failRuntimeCommit = true;
      await expectLater(
        harness.container.read(timerEngineProvider.notifier).finish(),
        throwsA(isA<StateError>()),
      );
      expect(await bridge.getActiveTimerSession(), isNull);

      final restored = await _Harness.create(
        bridge: bridge,
        clock: harness.clock,
        createTask: false,
      );
      addTearDown(restored.dispose);
      expect(
        (await restored.container.read(
          timerEngineProvider.future,
        )).completedWorkCycles,
        1,
      );
      final restarted = await _Harness.create(
        bridge: bridge,
        clock: harness.clock,
        createTask: false,
      );
      addTearDown(restarted.dispose);
      expect(
        (await restarted.container.read(
          timerEngineProvider.future,
        )).completedWorkCycles,
        1,
      );
    },
  );

  test(
    'notification failures and conditional discard conflict preserve timer state',
    () async {
      final gateway = _FakeTimerNotificationGateway(throwOnSchedule: true);
      final harness = await _Harness.create(gateway: gateway);
      addTearDown(harness.dispose);
      await harness.bridge.setFrontendSetting(
        key: timerSettingsKey,
        value: const TimerSettings(notificationsEnabled: true).encode(),
      );

      await harness.container
          .read(timerEngineProvider.notifier)
          .startPomodoro(taskId: harness.task.id);
      expect(
        harness.container.read(timerEngineProvider).requireValue.isRunning,
        isTrue,
      );
      final actual = await harness.bridge.getActiveTimerSession();
      await harness.bridge.discardActiveTimerSession(
        expectedSessionId: actual!.sessionId,
      );
      await harness.bridge.startActiveTimerSession(
        session: ActiveTimerSessionDto(
          sessionId: '00000000-0000-4000-8000-000000000099',
          taskId: harness.task.id,
          mode: TimerModeDto.stopwatch,
          phase: TimerPhaseDto.work,
          state: TimerRunStateDto.running,
          startedAt: harness.clock.now(),
          lastResumedAt: harness.clock.now(),
          accumulatedActiveMs: 0,
        ),
      );

      await expectLater(
        harness.container.read(timerEngineProvider.notifier).discard(),
        throwsA(isA<TimerActiveConflictException>()),
      );
      expect(
        harness.container
            .read(timerEngineProvider)
            .requireValue
            .active
            ?.sessionId,
        '00000000-0000-4000-8000-000000000099',
      );
    },
  );

  test(
    'Timer notification payload and IDs remain separate and best effort',
    () async {
      final gateway = _FakeTimerNotificationGateway();
      final service = TimerNotificationService(gateway);
      await service.initialize(
        const TimerNotificationContent(title: 'Timer', body: 'Finished'),
      );
      const sessionId = '00000000-0000-4000-8000-000000000001';
      await service.schedule(
        sessionId: sessionId,
        scheduledAt: DateTime.now().add(const Duration(minutes: 1)),
      );
      expect(gateway.scheduled, [notificationIdForTimer(sessionId)]);
      expect(notificationIdForTimer(sessionId), isNegative);
      expect(
        TimerNotificationPayload.decode(
          const TimerNotificationPayload(sessionId: sessionId).encode(),
        )?.sessionId,
        sessionId,
      );
    },
  );

  test('sync completion invalidates stale active timer state', () async {
    final harness = await _Harness.create();
    addTearDown(harness.dispose);
    await harness.container
        .read(timerEngineProvider.notifier)
        .startStopwatch(taskId: harness.task.id);
    await harness.bridge.accountLogin(
      email: 'timer@example.com',
      password: 'password',
    );
    harness.container.invalidate(accountProvider);
    await harness.container.read(accountProvider.future);
    await harness.container.read(syncStatusProvider.future);

    harness.bridge.clearActiveTimerForNextSync();
    await harness.container.read(syncStatusProvider.notifier).syncNow();

    expect(
      (await harness.container.read(timerEngineProvider.future)).active,
      isNull,
    );
  });

  test('completed timer persistence invokes the common sync trigger', () async {
    var triggers = 0;
    final harness = await _Harness.create(
      completedSessionSyncTrigger: () => triggers += 1,
    );
    addTearDown(harness.dispose);

    await harness.container
        .read(timerEngineProvider.notifier)
        .startStopwatch(taskId: harness.task.id);
    harness.clock.advance(const Duration(minutes: 1));
    await harness.container.read(timerEngineProvider.notifier).finish();

    expect(triggers, 1);
  });
}

class _Harness {
  _Harness({
    required this.bridge,
    required this.clock,
    required this.gateway,
    required this.container,
    required this.task,
  });

  final FakeBridgeService bridge;
  final _FakeClock clock;
  final _FakeTimerNotificationGateway gateway;
  final ProviderContainer container;
  final TaskDto task;

  static Future<_Harness> create({
    FakeBridgeService? bridge,
    _FakeClock? clock,
    _FakeTimerNotificationGateway? gateway,
    bool createTask = true,
    void Function()? completedSessionSyncTrigger,
  }) async {
    final actualBridge = bridge ?? FakeBridgeService();
    final actualClock = clock ?? _FakeClock(DateTime.utc(2026, 7, 13, 8));
    final actualGateway = gateway ?? _FakeTimerNotificationGateway();
    await actualGateway.initialize();
    final notifications = TimerNotificationService(actualGateway);
    await notifications.initialize(
      const TimerNotificationContent(title: 'Timer', body: 'Finished'),
    );
    TaskDto task;
    if (createTask) {
      final list = await actualBridge.createList(name: 'List', sortOrder: 'a0');
      task = await actualBridge.createTask(listId: list.id, title: 'Task');
    } else {
      final list = (await actualBridge.getLists()).first;
      task = (await actualBridge.getTasks(listId: list.id)).first;
    }
    final container = ProviderContainer(
      overrides: [
        bridgeServiceProvider.overrideWithValue(actualBridge),
        timerClockProvider.overrideWithValue(actualClock),
        timerNotificationGatewayProvider.overrideWithValue(actualGateway),
        timerNotificationServiceProvider.overrideWithValue(notifications),
        if (completedSessionSyncTrigger != null)
          completedTimerSyncTriggerProvider.overrideWithValue(
            completedSessionSyncTrigger,
          ),
      ],
    );
    await container.read(timerEngineProvider.future);
    return _Harness(
      bridge: actualBridge,
      clock: actualClock,
      gateway: actualGateway,
      container: container,
      task: task,
    );
  }

  void dispose() => container.dispose();
}

class _FakeClock implements TimerClock {
  _FakeClock(this.value);

  DateTime value;

  @override
  DateTime now() => value;

  void advance(Duration duration) {
    value = value.add(duration);
  }
}

class _FakeTimerNotificationGateway implements TimerNotificationGateway {
  _FakeTimerNotificationGateway({
    this.permissionsGranted = true,
    this.throwOnSchedule = false,
  });

  final bool permissionsGranted;
  final bool throwOnSchedule;
  int permissionRequests = 0;
  final scheduled = <int>[];
  final cancelled = <int>[];

  @override
  Future<void> initialize() async {}

  @override
  Future<bool> requestPermissions() async {
    permissionRequests += 1;
    return permissionsGranted;
  }

  @override
  Future<void> schedule({
    required int notificationId,
    required DateTime scheduledAt,
    required TimerNotificationContent content,
    required TimerNotificationPayload payload,
  }) async {
    if (throwOnSchedule) {
      throw StateError('notification unavailable');
    }
    scheduled.add(notificationId);
  }

  @override
  Future<void> cancel(int notificationId) async {
    cancelled.add(notificationId);
  }
}

class _FailingRuntimeBridge extends FakeBridgeService {
  bool failRuntimeCommit = false;
  bool failBreakCommit = false;

  @override
  Future<void> setFrontendSetting({
    required FrontendSettingKeyDto key,
    required String value,
  }) async {
    if (key == timerRuntimeKey &&
        failRuntimeCommit &&
        value.contains('"pending":null')) {
      failRuntimeCommit = false;
      throw StateError('simulated runtime commit failure');
    }
    if (key == timerRuntimeKey &&
        failBreakCommit &&
        value.contains('"pendingBreakStartCycle":null')) {
      failBreakCommit = false;
      throw StateError('simulated break commit failure');
    }
    await super.setFrontendSetting(key: key, value: value);
  }
}

import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart'
    show ProviderListenable, ProviderOrFamily;
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/timer/timer_notifications.dart';
import 'package:taskveil/src/timer/timer_settings.dart';

const timerRuntimeKey = FrontendSettingKeyDto.timerRuntime;
const _maximumActiveDuration = Duration(days: 7);

class TimerActiveConflictException implements Exception {
  const TimerActiveConflictException();

  @override
  String toString() => 'TimerActiveConflictException';
}

class TimerEngineStateException implements Exception {
  const TimerEngineStateException(this.operation);

  final String operation;

  @override
  String toString() => 'TimerEngineStateException($operation)';
}

abstract class TimerClock {
  DateTime now();
}

class SystemTimerClock implements TimerClock {
  const SystemTimerClock();

  @override
  DateTime now() => DateTime.now().toUtc();
}

class TimerEngineState {
  const TimerEngineState({
    this.active,
    this.elapsed = Duration.zero,
    this.remaining,
    this.targetReachedAt,
    this.nextBreakPhase = TimerPhaseDto.shortBreak,
    this.completedWorkCycles = 0,
    this.isBreakPending = false,
    this.lastCompletion,
    this.breakJustCompleted = false,
  });

  final ActiveTimerSessionDto? active;
  final Duration elapsed;
  final Duration? remaining;
  final DateTime? targetReachedAt;
  final TimerPhaseDto nextBreakPhase;
  final int completedWorkCycles;
  final bool isBreakPending;
  final CompletedTimerSessionDto? lastCompletion;
  final bool breakJustCompleted;

  bool get isIdle => active == null;
  bool get isRunning => active?.state == TimerRunStateDto.running;
  bool get isPaused => active?.state == TimerRunStateDto.paused;
  bool get isTargetReached => remaining != null && remaining! <= Duration.zero;

  TimerEngineState copyWith({
    ActiveTimerSessionDto? active,
    bool clearActive = false,
    Duration? elapsed,
    Duration? remaining,
    bool clearRemaining = false,
    DateTime? targetReachedAt,
    bool clearTargetReachedAt = false,
    TimerPhaseDto? nextBreakPhase,
    int? completedWorkCycles,
    bool? isBreakPending,
    CompletedTimerSessionDto? lastCompletion,
    bool clearLastCompletion = false,
    bool? breakJustCompleted,
    bool clearBreakJustCompleted = false,
  }) {
    return TimerEngineState(
      active: clearActive ? null : active ?? this.active,
      elapsed: elapsed ?? this.elapsed,
      remaining: clearRemaining ? null : remaining ?? this.remaining,
      targetReachedAt: clearTargetReachedAt
          ? null
          : targetReachedAt ?? this.targetReachedAt,
      nextBreakPhase: nextBreakPhase ?? this.nextBreakPhase,
      completedWorkCycles: completedWorkCycles ?? this.completedWorkCycles,
      isBreakPending: isBreakPending ?? this.isBreakPending,
      lastCompletion: clearLastCompletion
          ? null
          : lastCompletion ?? this.lastCompletion,
      breakJustCompleted: clearBreakJustCompleted
          ? false
          : breakJustCompleted ?? this.breakJustCompleted,
    );
  }
}

Duration completedTimerTotal(Iterable<CompletedTimerSessionDto> sessions) {
  return Duration(
    milliseconds: sessions.fold<int>(
      0,
      (total, session) => total + session.activeDurationMs,
    ),
  );
}

class TimerEngineController extends AsyncNotifier<TimerEngineState> {
  TimerEngineController(
    this._bridgeProvider,
    this._notificationServiceProvider,
    this._clockProvider,
    this._completedSessionsProviderForTask,
    this._completedSessionSyncTriggerProvider,
  );

  final ProviderListenable<BridgeService> _bridgeProvider;
  final ProviderListenable<TimerNotificationService>
  _notificationServiceProvider;
  final ProviderListenable<TimerClock> _clockProvider;
  final ProviderOrFamily Function(String taskId)
  _completedSessionsProviderForTask;
  final ProviderListenable<void Function()>
  _completedSessionSyncTriggerProvider;
  Timer? _displayTicker;
  var _commandInFlight = false;

  BridgeService get _bridge => ref.read(_bridgeProvider);
  TimerNotificationService get _notifications =>
      ref.read(_notificationServiceProvider);
  TimerClock get _clock => ref.read(_clockProvider);

  @override
  Future<TimerEngineState> build() async {
    ref.onDispose(() => _displayTicker?.cancel());
    final bridge = ref.watch(_bridgeProvider);
    ref.watch(_clockProvider);
    ref.watch(_notificationServiceProvider);
    var runtime = await _loadRuntime(bridge);
    var active = await bridge.getActiveTimerSession();
    runtime = await _reconcilePending(bridge, runtime, active);
    active = await bridge.getActiveTimerSession();
    runtime = await _reconcileBreakStart(bridge, runtime, active);
    if (active != null) {
      final settled = await _settleIfDue(bridge, active, runtime);
      runtime = settled.runtime;
      active = settled.active;
      final settings = await _loadSettings(bridge);
      final result = (await _snapshot(active, runtime, settings: settings))
          .copyWith(
            lastCompletion: settled.completion,
            breakJustCompleted: settled.breakCompleted,
          );
      _configureDisplayTicker(result);
      await _scheduleNotification(result, settings);
      return result;
    }
    final settings = await _loadSettings(bridge);
    final result = await _snapshot(active, runtime, settings: settings);
    _configureDisplayTicker(result);
    await _scheduleNotification(result, settings);
    return result;
  }

  Future<void> startPomodoro({required String taskId}) async {
    final settings = await _loadSettings(_bridge);
    await _start(
      taskId: taskId,
      mode: TimerModeDto.pomodoro,
      phase: TimerPhaseDto.work,
      target: Duration(minutes: settings.workMinutes),
      settings: settings,
    );
  }

  Future<void> startStopwatch({required String taskId}) async {
    await _start(
      taskId: taskId,
      mode: TimerModeDto.stopwatch,
      phase: TimerPhaseDto.work,
      settings: await _loadSettings(_bridge),
    );
  }

  Future<void> startBreak() async {
    final current = _requireState('startBreak');
    if (!current.isIdle) {
      throw const TimerActiveConflictException();
    }
    if (!current.isBreakPending) {
      throw const TimerEngineStateException('startBreak');
    }
    final settings = await _loadSettings(_bridge);
    final phase = _nextBreakPhase(current.completedWorkCycles, settings);
    final minutes = phase == TimerPhaseDto.longBreak
        ? settings.longBreakMinutes
        : settings.shortBreakMinutes;
    await _startPendingBreak(
      phase: phase,
      target: Duration(minutes: minutes),
      settings: settings,
    );
  }

  Future<void> skipBreak() => _runCommand(() async {
    final current = _requireState('skipBreak');
    if (!current.isIdle || !current.isBreakPending) {
      throw const TimerEngineStateException('skipBreak');
    }
    final runtime = (await _loadRuntime(_bridge)).copyWith(
      breakHandledThroughCycle: current.completedWorkCycles,
      clearPendingBreakStart: true,
    );
    await _saveRuntime(_bridge, runtime);
    await _publish(null, runtime: runtime);
  });

  Future<void> pause() => _runCommand(() async {
    var current = _requireActive('pause');
    if (current.state != TimerRunStateDto.running) {
      throw const TimerEngineStateException('pause');
    }
    final settled = await _settleIfDue(
      _bridge,
      current,
      await _loadRuntime(_bridge),
    );
    if (settled.active == null) {
      await _publishSettlement(settled);
      return;
    }
    current = settled.active!;
    final now = _clock.now();
    final updated = _copyActive(
      current,
      state: TimerRunStateDto.paused,
      clearLastResumedAt: true,
      accumulatedActiveMs: _elapsedAt(current, now).inMilliseconds,
    );
    await _bridge.updateActiveTimerSession(session: updated);
    await _notifications.cancel(current.sessionId);
    await _publish(updated);
  });

  Future<void> resume() => _runCommand(() async {
    var current = _requireActive('resume');
    if (current.state != TimerRunStateDto.paused) {
      throw const TimerEngineStateException('resume');
    }
    final runtime = await _loadRuntime(_bridge);
    final settled = await _settleIfDue(_bridge, current, runtime);
    if (settled.active == null) {
      await _publish(null, runtime: settled.runtime);
      if (settled.completion != null) {
        state = AsyncData(
          state.requireValue.copyWith(lastCompletion: settled.completion),
        );
      }
      return;
    }
    current = settled.active!;
    final updated = _copyActive(
      current,
      state: TimerRunStateDto.running,
      lastResumedAt: _clock.now(),
    );
    await _bridge.updateActiveTimerSession(session: updated);
    await _publish(updated);
  });

  Future<void> addTime(Duration amount) => _runCommand(() async {
    final current = _requireActive('addTime');
    if (current.mode != TimerModeDto.pomodoro ||
        current.targetDurationMs == null ||
        amount.inMinutes <= 0 ||
        amount.inMinutes % 5 != 0 ||
        amount.inMilliseconds != amount.inMinutes * 60000) {
      throw const TimerEngineStateException('addTime');
    }
    final target = current.targetDurationMs! + amount.inMilliseconds;
    if (target > _maximumActiveDuration.inMilliseconds) {
      throw const TimerEngineStateException('addTime');
    }
    final updated = _copyActive(current, targetDurationMs: target);
    await _bridge.updateActiveTimerSession(session: updated);
    await _publish(updated);
  });

  Future<CompletedTimerSessionDto?> finish({
    TimerFinishKindDto kind = TimerFinishKindDto.completed,
  }) async {
    CompletedTimerSessionDto? result;
    await _runCommand(() async {
      final active = _requireState('finish').active;
      if (active == null) {
        return;
      }
      final settled = await _settleIfDue(
        _bridge,
        active,
        await _loadRuntime(_bridge),
      );
      if (settled.active == null) {
        result = settled.completion;
        await _publishSettlement(settled);
        return;
      }
      await _notifications.cancel(active.sessionId);
      if (active.phase != TimerPhaseDto.work) {
        await _discardExpected(active);
        final runtime = await _loadRuntime(_bridge);
        await _publish(null, runtime: runtime);
        state = AsyncData(
          state.requireValue.copyWith(breakJustCompleted: true),
        );
        return;
      }
      final finished = await _finishWork(active, kind: kind);
      result = finished.completed;
      final settings = await _loadSettings(_bridge);
      final snapshot = await _snapshot(
        null,
        finished.runtime,
        settings: settings,
      );
      state = AsyncData(snapshot.copyWith(lastCompletion: result));
      _configureDisplayTicker(state.requireValue);
    });
    return result;
  }

  void clearLastOutcome() {
    final current = state.value;
    if (current == null ||
        (current.lastCompletion == null && !current.breakJustCompleted)) {
      return;
    }
    state = AsyncData(
      current.copyWith(
        clearLastCompletion: true,
        clearBreakJustCompleted: true,
      ),
    );
  }

  Future<void> discard() => _runCommand(() async {
    final active = _requireActive('discard');
    await _notifications.cancel(active.sessionId);
    await _discardExpected(active);
    var runtime = await _loadRuntime(_bridge);
    if (runtime.pending?.sessionId == active.sessionId) {
      runtime = runtime.copyWith(clearPending: true);
      await _saveRuntime(_bridge, runtime);
    }
    state = AsyncData(
      await _snapshot(null, runtime, settings: await _loadSettings(_bridge)),
    );
    _configureDisplayTicker(state.requireValue);
  });

  Future<void> settleOnResume() => _runCommand(() async {
    final active = await _bridge.getActiveTimerSession();
    var runtime = await _loadRuntime(_bridge);
    runtime = await _reconcilePending(_bridge, runtime, active);
    final restored = await _bridge.getActiveTimerSession();
    if (restored != null) {
      final settled = await _settleIfDue(_bridge, restored, runtime);
      runtime = settled.runtime;
      await _publish(settled.active, runtime: runtime);
      if (settled.completion != null) {
        final current = state.requireValue;
        state = AsyncData(current.copyWith(lastCompletion: settled.completion));
      } else if (settled.breakCompleted) {
        final current = state.requireValue;
        state = AsyncData(current.copyWith(breakJustCompleted: true));
      }
      return;
    }
    await _publish(restored, runtime: runtime);
  });

  Future<void> refreshDisplay() async {
    final current = state.value;
    final active = current?.active;
    if (current == null || active == null) {
      return;
    }
    final elapsed = _elapsedAt(active, _clock.now());
    final remaining = active.targetDurationMs == null
        ? null
        : Duration(milliseconds: active.targetDurationMs!) - elapsed;
    state = AsyncData(current.copyWith(elapsed: elapsed, remaining: remaining));
    if (active.mode == TimerModeDto.pomodoro &&
        active.state == TimerRunStateDto.running &&
        remaining != null &&
        remaining <= Duration.zero) {
      try {
        await settleOnResume();
      } catch (_) {
        // The durable active session remains the source of truth. A later
        // display tick, foreground resume, or restart retries settlement.
      }
    }
  }

  Future<void> _start({
    String? taskId,
    required TimerModeDto mode,
    required TimerPhaseDto phase,
    Duration? target,
    required TimerSettings settings,
  }) => _runCommand(() async {
    final now = _clock.now();
    final active = ActiveTimerSessionDto(
      sessionId: _newUuid(),
      taskId: taskId,
      mode: mode,
      phase: phase,
      state: TimerRunStateDto.running,
      startedAt: now,
      lastResumedAt: now,
      accumulatedActiveMs: 0,
      targetDurationMs: target?.inMilliseconds,
    );
    final outcome = await _bridge.startActiveTimerSession(session: active);
    if (outcome == ActiveTimerStartOutcomeDto.conflict) {
      await _publish(await _bridge.getActiveTimerSession());
      throw const TimerActiveConflictException();
    }
    await _publish(active, settings: settings);
  });

  Future<void> _startPendingBreak({
    required TimerPhaseDto phase,
    required Duration target,
    required TimerSettings settings,
  }) => _runCommand(() async {
    var runtime = await _loadRuntime(_bridge);
    final cycle = runtime.completedWorkCycles;
    if (cycle <= runtime.breakHandledThroughCycle) {
      throw const TimerEngineStateException('startBreak');
    }
    runtime = runtime.copyWith(pendingBreakStartCycle: cycle);
    await _saveRuntime(_bridge, runtime);
    final now = _clock.now();
    final active = ActiveTimerSessionDto(
      sessionId: _newUuid(),
      mode: TimerModeDto.pomodoro,
      phase: phase,
      state: TimerRunStateDto.running,
      startedAt: now,
      lastResumedAt: now,
      accumulatedActiveMs: 0,
      targetDurationMs: target.inMilliseconds,
    );
    final outcome = await _bridge.startActiveTimerSession(session: active);
    if (outcome == ActiveTimerStartOutcomeDto.conflict) {
      runtime = runtime.copyWith(clearPendingBreakStart: true);
      await _saveRuntime(_bridge, runtime);
      await _publish(await _bridge.getActiveTimerSession(), runtime: runtime);
      throw const TimerActiveConflictException();
    }
    final committed = runtime.copyWith(
      breakHandledThroughCycle: cycle,
      clearPendingBreakStart: true,
    );
    try {
      await _saveRuntime(_bridge, committed);
    } catch (_) {
      await _publish(active, runtime: runtime, settings: settings);
      rethrow;
    }
    await _publish(active, runtime: committed, settings: settings);
  });

  Future<_FinishResult> _finishWork(
    ActiveTimerSessionDto active, {
    required TimerFinishKindDto kind,
    DateTime? endedAt,
  }) async {
    final end = (endedAt ?? _clock.now()).toUtc();
    final duration = _elapsedAt(active, end);
    var runtime = await _loadRuntime(_bridge);
    if (duration <= Duration.zero) {
      await _discardExpected(active);
      return _FinishResult(completed: null, runtime: runtime);
    }
    final countsCycle =
        active.mode == TimerModeDto.pomodoro &&
        kind == TimerFinishKindDto.completed;
    if (countsCycle) {
      runtime = runtime.copyWith(
        pending: _PendingWorkCompletion(
          sessionId: active.sessionId,
          taskId: active.taskId!,
        ),
      );
      await _saveRuntime(_bridge, runtime);
    }
    final now = _clock.now();
    final createdAt = now.isBefore(end) ? end : now;
    final completed = CompletedTimerSessionDto(
      id: active.sessionId,
      taskId: active.taskId!,
      mode: active.mode,
      finishKind: kind,
      startedAt: active.startedAt,
      endedAt: end,
      activeDurationMs: duration.inMilliseconds,
      createdAt: createdAt,
    );
    final inserted = await _bridge.finishActiveTimerSession(session: completed);
    if (inserted) {
      ref.read(_completedSessionSyncTriggerProvider)();
    }
    if (countsCycle) {
      runtime = runtime.copyWith(
        completedWorkCycles: runtime.completedWorkCycles + 1,
        clearPending: true,
      );
      await _saveRuntime(_bridge, runtime);
    }
    ref.invalidate(_completedSessionsProviderForTask(active.taskId!));
    return _FinishResult(completed: completed, runtime: runtime);
  }

  Future<_SettledTimer> _settleIfDue(
    BridgeService bridge,
    ActiveTimerSessionDto active,
    _TimerRuntime runtime,
  ) async {
    final now = _clock.now();
    final lifespanReachedAt = active.startedAt.add(_maximumActiveDuration);
    DateTime? pomodoroReachedAt;
    if (active.mode == TimerModeDto.pomodoro &&
        active.state == TimerRunStateDto.running) {
      pomodoroReachedAt = await bridge.pomodoroTargetReachedAt(session: active);
    }
    final settleAt =
        pomodoroReachedAt != null &&
            pomodoroReachedAt.isBefore(lifespanReachedAt)
        ? pomodoroReachedAt
        : lifespanReachedAt;
    if (now.isBefore(settleAt)) {
      return _SettledTimer(active: active, runtime: runtime);
    }
    await _notifications.cancel(active.sessionId);
    if (active.phase != TimerPhaseDto.work) {
      await _discardExpected(active);
      return _SettledTimer(
        active: null,
        runtime: runtime,
        breakCompleted: true,
      );
    }
    final reachedPomodoroTarget =
        pomodoroReachedAt != null &&
        !pomodoroReachedAt.isAfter(lifespanReachedAt);
    final finished = await _finishWork(
      active,
      kind: reachedPomodoroTarget
          ? TimerFinishKindDto.completed
          : TimerFinishKindDto.interrupted,
      endedAt: settleAt,
    );
    return _SettledTimer(
      active: null,
      runtime: finished.runtime,
      completion: finished.completed,
    );
  }

  Future<_TimerRuntime> _reconcilePending(
    BridgeService bridge,
    _TimerRuntime runtime,
    ActiveTimerSessionDto? active,
  ) async {
    final pending = runtime.pending;
    if (pending == null) {
      return runtime;
    }
    final completed = await bridge.getCompletedTimerSessions(
      taskId: pending.taskId,
    );
    if (completed.any((session) => session.id == pending.sessionId)) {
      final reconciled = runtime.copyWith(
        completedWorkCycles: runtime.completedWorkCycles + 1,
        clearPending: true,
      );
      await _saveRuntime(bridge, reconciled);
      return reconciled;
    }
    if (active?.sessionId == pending.sessionId) {
      return runtime;
    }
    final cleared = runtime.copyWith(clearPending: true);
    await _saveRuntime(bridge, cleared);
    return cleared;
  }

  Future<_TimerRuntime> _reconcileBreakStart(
    BridgeService bridge,
    _TimerRuntime runtime,
    ActiveTimerSessionDto? active,
  ) async {
    final pendingCycle = runtime.pendingBreakStartCycle;
    if (pendingCycle == null) {
      return runtime;
    }
    final activeIsBreak =
        active != null &&
        active.mode == TimerModeDto.pomodoro &&
        active.phase != TimerPhaseDto.work;
    final reconciled = runtime.copyWith(
      breakHandledThroughCycle: activeIsBreak
          ? max(runtime.breakHandledThroughCycle, pendingCycle)
          : runtime.breakHandledThroughCycle,
      clearPendingBreakStart: true,
    );
    await _saveRuntime(bridge, reconciled);
    return reconciled;
  }

  Future<void> _discardExpected(ActiveTimerSessionDto expected) async {
    final discarded = await _bridge.discardActiveTimerSession(
      expectedSessionId: expected.sessionId,
    );
    if (discarded) {
      return;
    }
    final actual = await _bridge.getActiveTimerSession();
    await _publish(actual);
    throw const TimerActiveConflictException();
  }

  Future<void> _publish(
    ActiveTimerSessionDto? active, {
    _TimerRuntime? runtime,
    TimerSettings? settings,
  }) async {
    final actualRuntime = runtime ?? await _loadRuntime(_bridge);
    final actualSettings = settings ?? await _loadSettings(_bridge);
    final snapshot = await _snapshot(
      active,
      actualRuntime,
      settings: actualSettings,
    );
    state = AsyncData(snapshot);
    _configureDisplayTicker(snapshot);
    await _scheduleNotification(snapshot, actualSettings);
  }

  Future<void> _publishSettlement(_SettledTimer settled) async {
    await _publish(settled.active, runtime: settled.runtime);
    final completion = settled.completion;
    if (completion != null || settled.breakCompleted) {
      state = AsyncData(
        state.requireValue.copyWith(
          lastCompletion: completion,
          breakJustCompleted: settled.breakCompleted,
        ),
      );
    }
  }

  Future<TimerEngineState> _snapshot(
    ActiveTimerSessionDto? active,
    _TimerRuntime runtime, {
    required TimerSettings settings,
  }) async {
    if (active == null) {
      return TimerEngineState(
        completedWorkCycles: runtime.completedWorkCycles,
        isBreakPending:
            runtime.completedWorkCycles > runtime.breakHandledThroughCycle,
        nextBreakPhase: _nextBreakPhase(runtime.completedWorkCycles, settings),
      );
    }
    final elapsed = _elapsedAt(active, _clock.now());
    final remaining = active.targetDurationMs == null
        ? null
        : Duration(milliseconds: active.targetDurationMs!) - elapsed;
    final reachedAt =
        active.mode == TimerModeDto.pomodoro &&
            active.state == TimerRunStateDto.running
        ? await _bridge.pomodoroTargetReachedAt(session: active)
        : null;
    return TimerEngineState(
      active: active,
      elapsed: elapsed,
      remaining: remaining,
      targetReachedAt: reachedAt,
      completedWorkCycles: runtime.completedWorkCycles,
      isBreakPending:
          runtime.completedWorkCycles > runtime.breakHandledThroughCycle,
      nextBreakPhase: _nextBreakPhase(runtime.completedWorkCycles, settings),
    );
  }

  Future<void> _scheduleNotification(
    TimerEngineState snapshot,
    TimerSettings settings,
  ) async {
    final active = snapshot.active;
    final target = snapshot.targetReachedAt;
    if (!settings.notificationsEnabled ||
        active == null ||
        active.state != TimerRunStateDto.running ||
        target == null) {
      return;
    }
    await _notifications.schedule(
      sessionId: active.sessionId,
      scheduledAt: target,
    );
  }

  void _configureDisplayTicker(TimerEngineState snapshot) {
    _displayTicker?.cancel();
    _displayTicker = null;
    if (!snapshot.isRunning) {
      return;
    }
    _displayTicker = Timer.periodic(
      const Duration(seconds: 1),
      (_) => unawaited(refreshDisplay()),
    );
  }

  Future<void> _runCommand(Future<void> Function() command) async {
    if (_commandInFlight) {
      throw const TimerEngineStateException('commandInFlight');
    }
    _commandInFlight = true;
    try {
      await command();
    } finally {
      _commandInFlight = false;
    }
  }

  TimerEngineState _requireState(String operation) {
    final current = state.value;
    if (current == null) {
      throw TimerEngineStateException(operation);
    }
    return current;
  }

  ActiveTimerSessionDto _requireActive(String operation) {
    final active = _requireState(operation).active;
    if (active == null) {
      throw TimerEngineStateException(operation);
    }
    return active;
  }
}

Duration _elapsedAt(ActiveTimerSessionDto active, DateTime now) {
  var milliseconds = active.accumulatedActiveMs;
  final resumed = active.lastResumedAt;
  if (active.state == TimerRunStateDto.running && resumed != null) {
    milliseconds += max(0, now.difference(resumed).inMilliseconds);
  }
  return Duration(
    milliseconds: min(milliseconds, _maximumActiveDuration.inMilliseconds),
  );
}

TimerPhaseDto _nextBreakPhase(int completedCycles, TimerSettings settings) {
  if (completedCycles > 0 && completedCycles % settings.longBreakEvery == 0) {
    return TimerPhaseDto.longBreak;
  }
  return TimerPhaseDto.shortBreak;
}

ActiveTimerSessionDto _copyActive(
  ActiveTimerSessionDto value, {
  TimerRunStateDto? state,
  DateTime? lastResumedAt,
  bool clearLastResumedAt = false,
  int? accumulatedActiveMs,
  int? targetDurationMs,
}) {
  return ActiveTimerSessionDto(
    sessionId: value.sessionId,
    taskId: value.taskId,
    mode: value.mode,
    phase: value.phase,
    state: state ?? value.state,
    startedAt: value.startedAt,
    lastResumedAt: clearLastResumedAt
        ? null
        : lastResumedAt ?? value.lastResumedAt,
    accumulatedActiveMs: accumulatedActiveMs ?? value.accumulatedActiveMs,
    targetDurationMs: targetDurationMs ?? value.targetDurationMs,
  );
}

Future<TimerSettings> _loadSettings(BridgeService bridge) async {
  final persisted = await bridge.getFrontendSetting(key: timerSettingsKey);
  if (persisted == null) {
    return const TimerSettings();
  }
  try {
    return TimerSettings.decode(persisted);
  } on TimerSettingsValidationException {
    return const TimerSettings();
  }
}

Future<_TimerRuntime> _loadRuntime(BridgeService bridge) async {
  final persisted = await bridge.getFrontendSetting(key: timerRuntimeKey);
  if (persisted == null) {
    return const _TimerRuntime();
  }
  try {
    return _TimerRuntime.decode(persisted);
  } catch (_) {
    return const _TimerRuntime();
  }
}

Future<void> _saveRuntime(BridgeService bridge, _TimerRuntime runtime) {
  return bridge.setFrontendSetting(
    key: timerRuntimeKey,
    value: runtime.encode(),
  );
}

class _TimerRuntime {
  /// The pending record is the first half of a two-phase local journal.
  ///
  /// It is written before Rust atomically finishes a Pomodoro work session.
  /// The cycle count and pending clear are written afterwards. A restart can
  /// query immutable completed sessions by ID and perform the second half
  /// exactly once if the process died between those writes.
  const _TimerRuntime({
    this.completedWorkCycles = 0,
    this.breakHandledThroughCycle = 0,
    this.pending,
    this.pendingBreakStartCycle,
  });

  final int completedWorkCycles;
  final int breakHandledThroughCycle;
  final _PendingWorkCompletion? pending;
  final int? pendingBreakStartCycle;

  _TimerRuntime copyWith({
    int? completedWorkCycles,
    int? breakHandledThroughCycle,
    _PendingWorkCompletion? pending,
    bool clearPending = false,
    int? pendingBreakStartCycle,
    bool clearPendingBreakStart = false,
  }) {
    return _TimerRuntime(
      completedWorkCycles: completedWorkCycles ?? this.completedWorkCycles,
      breakHandledThroughCycle:
          breakHandledThroughCycle ?? this.breakHandledThroughCycle,
      pending: clearPending ? null : pending ?? this.pending,
      pendingBreakStartCycle: clearPendingBreakStart
          ? null
          : pendingBreakStartCycle ?? this.pendingBreakStartCycle,
    );
  }

  String encode() => jsonEncode({
    'version': 1,
    'completedWorkCycles': completedWorkCycles,
    'breakHandledThroughCycle': breakHandledThroughCycle,
    'pending': pending?.toJson(),
    'pendingBreakStartCycle': pendingBreakStartCycle,
  });

  static _TimerRuntime decode(String source) {
    final value = jsonDecode(source);
    if (value is! Map<String, Object?> || value['version'] != 1) {
      throw const FormatException();
    }
    final cycles = value['completedWorkCycles'];
    if (cycles is! int || cycles < 0) {
      throw const FormatException();
    }
    final handled = value['breakHandledThroughCycle'] ?? 0;
    final pendingBreak = value['pendingBreakStartCycle'];
    if (handled is! int ||
        handled < 0 ||
        handled > cycles ||
        (pendingBreak != null &&
            (pendingBreak is! int ||
                pendingBreak <= handled ||
                pendingBreak > cycles))) {
      throw const FormatException();
    }
    return _TimerRuntime(
      completedWorkCycles: cycles,
      breakHandledThroughCycle: handled,
      pending: _PendingWorkCompletion.fromJson(value['pending']),
      pendingBreakStartCycle: pendingBreak as int?,
    );
  }
}

class _PendingWorkCompletion {
  const _PendingWorkCompletion({required this.sessionId, required this.taskId});

  final String sessionId;
  final String taskId;

  Map<String, Object> toJson() => {'sessionId': sessionId, 'taskId': taskId};

  static _PendingWorkCompletion? fromJson(Object? value) {
    if (value == null) {
      return null;
    }
    if (value is! Map<String, Object?> ||
        value['sessionId'] is! String ||
        value['taskId'] is! String) {
      throw const FormatException();
    }
    return _PendingWorkCompletion(
      sessionId: value['sessionId']! as String,
      taskId: value['taskId']! as String,
    );
  }
}

class _SettledTimer {
  const _SettledTimer({
    required this.active,
    required this.runtime,
    this.completion,
    this.breakCompleted = false,
  });

  final ActiveTimerSessionDto? active;
  final _TimerRuntime runtime;
  final CompletedTimerSessionDto? completion;
  final bool breakCompleted;
}

class _FinishResult {
  const _FinishResult({required this.completed, required this.runtime});

  final CompletedTimerSessionDto? completed;
  final _TimerRuntime runtime;
}

String _newUuid() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  final hex = bytes
      .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
      .join();
  return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-'
      '${hex.substring(12, 16)}-${hex.substring(16, 20)}-'
      '${hex.substring(20)}';
}

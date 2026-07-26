import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/core/civil_time.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/core/providers.dart'
    show defaultSyncServerUrl, onboardingCompletedSettingKey;
import 'package:taskveil/src/rust/api.dart';

TaskDueDto testDateOnlyDueFromMillis(int value) =>
    dateOnlyDue(DateTime.fromMillisecondsSinceEpoch(value));

TaskDueDto testDateTimeDueFromMillis(int value, {String timeZone = 'UTC'}) =>
    dateTimeDueFromInstant(
      DateTime.fromMillisecondsSinceEpoch(value),
      timeZone: timeZone,
    );

/// In-memory fake [BridgeService].
///
/// Widget tests use this instead of [FrbBridgeService] so the whole
/// screen/provider/router skeleton can be exercised without the native Rust
/// library and without calling `initCore`.
class FakeBridgeService implements BridgeService {
  FakeBridgeService({
    bool onboardingCompleted = true,
    this.accountRegistrationGate,
    this.restoredRegistrationState,
    this.registrationStateFailures = 0,
    this.recoveryKeyFailures = 0,
  }) : _settings = {
         if (onboardingCompleted) onboardingCompletedSettingKey: '1',
       };

  final Future<void>? accountRegistrationGate;
  AccountRegistrationStateDto? restoredRegistrationState;
  int registrationStateFailures;
  int recoveryKeyFailures;
  final List<ListDto> _lists = [];
  final List<TaskDto> _tasks = [];
  final List<TemplateDto> _templates = [];
  final List<TaskSeriesDto> _series = [];
  final List<ReminderDto> _reminders = [];
  final List<CompletedTimerSessionDto> _completedTimerSessions = [];
  final List<FakeTaskUndoEntry> _undoEntries = [];
  final Map<FrontendSettingKeyDto, String> _settings;
  String _syncServerUrl = defaultSyncServerUrl;
  final List<void Function()> _pendingSyncMutations = [];
  final List<FakeReorderCall> reorderCalls = [];
  final List<String> updateTaskCalls = [];
  int syncNowCalls = 0;
  int realtimeTicketCalls = 0;
  int organizationSafetyConfirmCalls = 0;
  String? _pendingRegistrationEmail;
  String? _pendingRecoveryKey;
  AccountSessionStateDto _accountSession = const AccountSessionStateDto(
    loggedIn: false,
    recoveryPending: false,
  );
  SyncStatusDto _syncStatus = const SyncStatusDto(
    loggedIn: false,
    running: false,
    pushedCount: 0,
    pushAckedCount: 0,
    pushSupersededCount: 0,
    pulledCount: 0,
    appliedCount: 0,
    deletedCount: 0,
    decryptFailedCount: 0,
    repushCount: 0,
    missingKeyQuarantinedCount: 0,
    corruptionQuarantinedCount: 0,
    resolvedQuarantineCount: 0,
    upgradeRequired: false,
  );
  int _listSeq = 0;
  int _taskSeq = 0;
  int _templateSeq = 0;
  int _scheduleSeq = 0;
  int _reminderSeq = 0;
  int _undoSeq = 0;
  int _accountSeq = 0;
  ActiveTimerSessionDto? _activeTimerSession;

  void seedRecoveryPendingAccount(String email) {
    _accountSession = _newAccountSession(email, recoveryPending: true);
    _pendingRecoveryKey =
        'amber anchor apricot atlas bamboo beacon birch breeze cabin cedar cinder cobalt coral cotton dawn delta ember fern flint garden harbor hazel indigo juniper';
  }

  FakeLargeSeedSummary seedLargeDataset({
    int listCount = 10,
    int tasksPerList = 1000,
    int? todayStartMs,
  }) {
    _lists.clear();
    _tasks.clear();
    _templates.clear();
    _series.clear();
    _reminders.clear();
    _completedTimerSessions.clear();
    _activeTimerSession = null;
    _undoEntries.clear();
    _pendingSyncMutations.clear();
    reorderCalls.clear();
    updateTaskCalls.clear();
    syncNowCalls = 0;
    realtimeTicketCalls = 0;
    organizationSafetyConfirmCalls = 0;
    _settings.clear();
    _settings[onboardingCompletedSettingKey] = '1';
    _accountSession = const AccountSessionStateDto(
      loggedIn: false,
      recoveryPending: false,
    );
    _syncStatus = const SyncStatusDto(
      loggedIn: false,
      running: false,
      pushedCount: 0,
      pushAckedCount: 0,
      pushSupersededCount: 0,
      pulledCount: 0,
      appliedCount: 0,
      deletedCount: 0,
      decryptFailedCount: 0,
      repushCount: 0,
      missingKeyQuarantinedCount: 0,
      corruptionQuarantinedCount: 0,
      resolvedQuarantineCount: 0,
      upgradeRequired: false,
    );
    _listSeq = listCount;
    _taskSeq = listCount * tasksPerList;
    _templateSeq = 0;
    _scheduleSeq = 0;
    _reminderSeq = 0;
    _undoSeq = 0;
    _accountSeq = 0;

    const rootTasksPerList = 700;
    const childTasksPerList = 220;
    final now = DateTime.now();
    final todayStart =
        todayStartMs ?? localCivilDay(now).millisecondsSinceEpoch;
    final tomorrowStart = todayStartMs == null
        ? localCivilDay(now, dayOffset: 1).millisecondsSinceEpoch
        : todayStart + _fakeDayMs;
    var dueTaskCount = 0;
    var closedTaskCount = 0;

    for (var listIndex = 0; listIndex < listCount; listIndex += 1) {
      final now = _fakeTimestamp(listIndex);
      _lists.add(
        ListDto(
          id: 'large-list-$listIndex',
          name: 'Performance List ${listIndex + 1}',
          color: '',
          icon: '',
          sortOrder: 'a${listIndex.toString().padLeft(2, '0')}',
          isDefault: listIndex == 0,
          createdAt: now,
          updatedAt: now,
        ),
      );
    }

    for (var listIndex = 0; listIndex < listCount; listIndex += 1) {
      final listId = 'large-list-$listIndex';
      final rootIds = <String>[];
      final childIds = <String>[];
      for (var taskIndex = 0; taskIndex < tasksPerList; taskIndex += 1) {
        final globalIndex = (listIndex * tasksPerList) + taskIndex;
        final taskId = 'large-task-$globalIndex';
        String? parentTaskId;
        if (taskIndex < rootTasksPerList) {
          rootIds.add(taskId);
        } else if (taskIndex < rootTasksPerList + childTasksPerList) {
          parentTaskId =
              rootIds[(taskIndex - rootTasksPerList) % rootIds.length];
          childIds.add(taskId);
        } else {
          parentTaskId =
              childIds[(taskIndex - rootTasksPerList - childTasksPerList) %
                  childIds.length];
        }
        final status = switch (globalIndex % 10) {
          0 => 'done',
          1 => 'wont_do',
          2 || 3 => 'in_progress',
          _ => 'todo',
        };
        final dueAt = switch (globalIndex % 6) {
          0 => null,
          1 => todayStart - _fakeDayMs,
          2 => todayStart + ((globalIndex % 12) * _fakeHourMs),
          3 => tomorrowStart + ((globalIndex % 8) * _fakeHourMs),
          4 => tomorrowStart + (7 * _fakeDayMs),
          _ => null,
        };
        if (dueAt != null) {
          dueTaskCount += 1;
        }
        final isClosed = status == 'done' || status == 'wont_do';
        final completedAt = isClosed
            ? globalIndex % 4 == 0
                  ? todayStart + ((globalIndex % 10) * _fakeTenMinuteMs)
                  : todayStart - (2 * _fakeDayMs)
            : null;
        if (isClosed) {
          closedTaskCount += 1;
        }
        final keyword = globalIndex % 17 == 0
            ? 'alpha'
            : globalIndex % 19 == 0
            ? '日本語'
            : 'routine';
        _tasks.add(
          TaskDto(
            id: taskId,
            listId: listId,
            parentTaskId: parentTaskId,
            title: 'Task ${globalIndex.toString().padLeft(5, '0')} $keyword',
            note: 'Seeded note ${globalIndex.toString().padLeft(5, '0')}',
            status: status,
            priority: globalIndex % 4,
            due: dueAt == null ? null : testDateOnlyDueFromMillis(dueAt),
            scheduledAt: dueAt == null ? null : dueAt - _fakeHourMs,
            estimatedMinutes: 15 + ((globalIndex % 6) * 10),
            sortOrder: 'a${taskIndex.toString().padLeft(4, '0')}',
            completedAt: completedAt,
            closedReason: status == 'wont_do' ? 'not_now' : null,
            createdAt: _fakeTimestamp(100 + globalIndex),
            updatedAt: _fakeTimestamp(200 + globalIndex),
          ),
        );
      }
    }

    return FakeLargeSeedSummary(
      listCount: listCount,
      taskCount: listCount * tasksPerList,
      dueTaskCount: dueTaskCount,
      closedTaskCount: closedTaskCount,
      defaultListId: _lists.first.id,
    );
  }

  @override
  Future<AccountSessionStateDto> getAccountSessionState() async {
    return _accountSession;
  }

  @override
  Future<AccountRegistrationPendingDto> accountRegistrationBegin({
    required String email,
    String? serverUrl,
  }) async {
    final registrationGate = accountRegistrationGate;
    if (registrationGate != null) {
      await registrationGate;
    }
    if (email.trim().isEmpty) {
      throw Exception('account request failed');
    }
    if (serverUrl != null && serverUrl.trim().isNotEmpty) {
      await setSyncServerUrl(serverUrl: serverUrl.trim());
    }
    _pendingRegistrationEmail = email.trim();
    final now = DateTime.now().millisecondsSinceEpoch;
    restoredRegistrationState = AccountRegistrationStateDto(
      phase: 'otp',
      email: email.trim(),
      expiresAtMs: now + 600000,
      nextRetryAtMs: now,
      canCancel: true,
    );
    return AccountRegistrationPendingDto(
      email: email.trim(),
      expiresAtMs: now + 600000,
      nextRetryAtMs: now,
    );
  }

  @override
  Future<AccountRegistrationStateDto?> accountRegistrationState() async {
    if (registrationStateFailures > 0) {
      registrationStateFailures -= 1;
      throw Exception('account request failed');
    }
    return restoredRegistrationState;
  }

  @override
  Future<void> accountRegistrationCancel() async {
    restoredRegistrationState = null;
    _pendingRegistrationEmail = null;
  }

  @override
  Future<AccountRegistrationPendingDto> accountRegistrationResend() async {
    final email = _pendingRegistrationEmail;
    if (email == null) throw Exception('account request failed');
    final now = DateTime.now().millisecondsSinceEpoch;
    return AccountRegistrationPendingDto(
      email: email,
      expiresAtMs: now + 600000,
      nextRetryAtMs: now + 60000,
    );
  }

  @override
  Future<void> accountRegistrationVerifyOtp({required String otp}) async {
    if (_pendingRegistrationEmail == null || otp.length != 8) {
      throw Exception('account request failed');
    }
    final email = _pendingRegistrationEmail!;
    restoredRegistrationState = AccountRegistrationStateDto(
      phase: 'password',
      email: email,
      expiresAtMs: DateTime.now().millisecondsSinceEpoch + 300000,
      canCancel: false,
    );
  }

  @override
  Future<AccountAuthResultDto> accountRegistrationComplete({
    required String password,
    String? deviceName,
  }) async {
    final email = _pendingRegistrationEmail;
    if (email == null || password.isEmpty) {
      throw Exception('account request failed');
    }
    final session = _newAccountSession(email, recoveryPending: true);
    _accountSession = session;
    _syncStatus = _copySyncStatus(_syncStatus, loggedIn: true);
    _pendingRecoveryKey =
        'amber anchor apricot atlas bamboo beacon birch breeze cabin cedar cinder cobalt coral cotton dawn delta ember fern flint garden harbor hazel indigo juniper';
    restoredRegistrationState = null;
    return AccountAuthResultDto(
      session: session,
      recoveryKey: _pendingRecoveryKey,
    );
  }

  @override
  Future<void> accountRegistrationAckRecoveryKey() async {
    _pendingRecoveryKey = null;
    final session = _accountSession;
    _accountSession = AccountSessionStateDto(
      loggedIn: session.loggedIn,
      email: session.email,
      userId: session.userId,
      tenantId: session.tenantId,
      deviceId: session.deviceId,
      recoveryPending: false,
    );
  }

  @override
  Future<String?> accountRegistrationRecoveryKey() async {
    if (recoveryKeyFailures > 0) {
      recoveryKeyFailures -= 1;
      throw Exception('account request failed');
    }
    return _pendingRecoveryKey;
  }

  @override
  Future<AccountAuthResultDto> accountLogin({
    required String email,
    required String password,
    String? serverUrl,
    String? deviceName,
  }) async {
    if (email.trim().isEmpty || password.isEmpty) {
      throw Exception('account request failed');
    }
    if (serverUrl != null && serverUrl.trim().isNotEmpty) {
      await setSyncServerUrl(serverUrl: serverUrl.trim());
    }
    final session = _newAccountSession(email.trim());
    _accountSession = session;
    _syncStatus = _copySyncStatus(_syncStatus, loggedIn: true);
    return AccountAuthResultDto(session: session);
  }

  @override
  Future<void> accountLogout() async {
    if (_pendingRecoveryKey != null) {
      throw Exception('recovery acknowledgement required');
    }
    _accountSession = const AccountSessionStateDto(
      loggedIn: false,
      recoveryPending: false,
    );
    _syncStatus = _copySyncStatus(_syncStatus, loggedIn: false);
  }

  @override
  Future<OrganizationSafetyStateDto> organizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
  }) async => OrganizationSafetyStateDto(
    ownerUserId: _accountSession.userId ?? 'owner-user',
    memberUserId: memberUserId,
    digest: 'test-safety-digest',
    decimal: '123456789012345678901234567890123456789012345678901234567890',
    qrPayload: 'AXRvZG9yaS10ZXN0LXNhZmV0eS1udW1iZXI=',
    verificationState: 'unverified',
    ownerConfirmed: false,
    memberConfirmed: false,
  );

  @override
  Future<OrganizationSafetyStateDto> confirmOrganizationSafetyNumber({
    required String tenantId,
    required String memberUserId,
    required String digest,
  }) async {
    organizationSafetyConfirmCalls += 1;
    final state = await organizationSafetyNumber(
      tenantId: tenantId,
      memberUserId: memberUserId,
    );
    if (digest != state.digest) {
      throw Exception('Safety number changed');
    }
    return OrganizationSafetyStateDto(
      ownerUserId: state.ownerUserId,
      memberUserId: state.memberUserId,
      digest: state.digest,
      decimal: state.decimal,
      qrPayload: state.qrPayload,
      verificationState: 'unverified',
      ownerConfirmed: true,
      memberConfirmed: false,
    );
  }

  @override
  Future<SyncStatusDto> getSyncStatus() async {
    return _syncStatus;
  }

  @override
  Future<SyncStatusDto> syncNow() async {
    syncNowCalls += 1;
    if (!_accountSession.loggedIn) {
      _syncStatus = _copySyncStatus(_syncStatus, loggedIn: false);
      return _syncStatus;
    }
    final syncMutations = List<void Function()>.of(_pendingSyncMutations);
    _pendingSyncMutations.clear();
    for (final mutation in syncMutations) {
      mutation();
    }
    _syncStatus = _copySyncStatus(
      _syncStatus,
      loggedIn: true,
      running: false,
      lastSuccessAt: _fakeTimestamp(_accountSeq + _taskSeq + _listSeq + 1),
      lastError: null,
    );
    return _syncStatus;
  }

  @override
  Future<SyncNowOutcomeDto> syncNowOutcome() async =>
      SyncNowOutcomeDto.synced(status: await syncNow());

  @override
  Future<BillingStateDto> billingBootstrap() =>
      Future.error(UnimplementedError('fake billing'));

  @override
  Future<BillingStateDto> refreshBilling() =>
      Future.error(UnimplementedError('fake billing'));

  @override
  Future<BillingStateDto?> getCachedBilling() async => null;

  @override
  Future<RealtimeTicketDto> getRealtimeTicket() async {
    if (!_accountSession.loggedIn) {
      throw Exception('account request failed');
    }
    realtimeTicketCalls += 1;
    return RealtimeTicketDto(
      websocketUrl: 'wss://realtime.example/v1/connect',
      ticket: 'opaque-test-ticket-$realtimeTicketCalls',
      expiresAt: DateTime.now().toUtc().add(const Duration(minutes: 5)),
    );
  }

  void addRemoteTaskForNextSync({
    required String listId,
    required String title,
    String? parentTaskId,
    int? dueAt,
    String note = '',
  }) {
    _pendingSyncMutations.add(() {
      final taskSeq = _taskSeq++;
      _tasks.add(
        TaskDto(
          id: 'remote-task-$taskSeq',
          listId: listId,
          parentTaskId: parentTaskId,
          title: title,
          note: note,
          status: 'todo',
          priority: 0,
          due: dueAt == null ? null : testDateOnlyDueFromMillis(dueAt),
          sortOrder: 'remote-$taskSeq',
          createdAt: _fakeTimestamp(3000 + taskSeq),
          updatedAt: _fakeTimestamp(3000 + taskSeq),
        ),
      );
    });
  }

  void clearActiveTimerForNextSync() {
    _pendingSyncMutations.add(() {
      _activeTimerSession = null;
    });
  }

  @override
  Future<String> getSyncServerUrl() async {
    return _syncServerUrl;
  }

  @override
  Future<void> setSyncServerUrl({required String serverUrl}) async {
    _syncServerUrl = serverUrl;
  }

  @override
  Future<String> getLocalTimeZone() async => 'UTC';

  @override
  Future<ListDto> createList({
    required String name,
    required String sortOrder,
  }) async {
    return _createList(name: name, sortOrder: sortOrder, isDefault: false);
  }

  Future<ListDto> createDefaultList({
    required String name,
    required String sortOrder,
  }) async {
    return _createList(name: name, sortOrder: sortOrder, isDefault: true);
  }

  Future<ListDto> _createList({
    required String name,
    required String sortOrder,
    required bool isDefault,
  }) async {
    final listSeq = _listSeq++;
    final now = _fakeTimestamp(listSeq);
    final list = ListDto(
      id: 'list-$listSeq',
      name: name,
      color: '',
      icon: '',
      sortOrder: sortOrder,
      isDefault: isDefault,
      createdAt: now,
      updatedAt: now,
    );
    _lists.add(list);
    return list;
  }

  @override
  Future<List<ListDto>> getLists() async {
    final active = _lists
        .where((list) => list.archivedAt == null)
        .toList(growable: false);
    active.sort(_compareLists);
    return List.unmodifiable(active);
  }

  @override
  Future<List<ListDto>> getArchivedLists() async {
    final archived = _lists
        .where((list) => list.archivedAt != null)
        .toList(growable: false);
    archived.sort((a, b) {
      final archivedAt = b.archivedAt!.compareTo(a.archivedAt!);
      if (archivedAt != 0) {
        return archivedAt;
      }
      return a.sortOrder.compareTo(b.sortOrder);
    });
    return List.unmodifiable(archived);
  }

  @override
  Future<List<TemplateDto>> getTemplates() async =>
      List.unmodifiable(_templates);

  @override
  Future<List<TaskSeriesDto>> getTaskSeries() async =>
      List.unmodifiable(_series);

  @override
  Future<String> validateRecurrenceRule({
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) async {
    final supportedFrequency = RegExp(
      r'^FREQ=(DAILY|WEEKLY|MONTHLY|YEARLY)(;|$)',
    ).hasMatch(rrule);
    if (!supportedFrequency || timeZone.isEmpty) {
      throw Exception('invalid recurrence rule');
    }
    return rrule;
  }

  @override
  Future<TemplateDto> saveTaskAsTemplate({
    required String taskId,
    required String name,
    String? defaultListId,
  }) async {
    final task = _tasks.firstWhere((task) => task.id == taskId);
    final now = _fakeTimestamp(5000 + _templateSeq);
    final template = TemplateDto(
      id: 'template-${_templateSeq++}',
      name: name,
      defaultListId: defaultListId,
      blueprintRevision: 'template-revision-$now',
      nodes: [
        TaskBlueprintNodeDto(
          nodeKey: task.id,
          siblingOrder: 0,
          title: task.title,
          note: task.note,
          priority: task.priority,
          estimatedMinutes: task.estimatedMinutes,
        ),
      ],
      createdAt: now,
      updatedAt: now,
    );
    _templates.add(template);
    return template;
  }

  @override
  Future<TemplateDto> createTemplate({
    required String name,
    String? defaultListId,
    required List<TaskBlueprintNodeDto> nodes,
  }) async {
    final now = _fakeTimestamp(5000 + _templateSeq);
    final template = TemplateDto(
      id: 'template-${_templateSeq++}',
      name: name,
      defaultListId: defaultListId,
      blueprintRevision: 'template-revision-$now',
      nodes: List.unmodifiable(nodes),
      createdAt: now,
      updatedAt: now,
    );
    _templates.add(template);
    return template;
  }

  @override
  Future<TemplateDto> updateTemplate({
    required String templateId,
    required String name,
    String? defaultListId,
    required List<TaskBlueprintNodeDto> nodes,
  }) async {
    final index = _templates.indexWhere((value) => value.id == templateId);
    final before = _templates[index];
    final updated = TemplateDto(
      id: before.id,
      name: name,
      defaultListId: defaultListId,
      blueprintRevision: '${before.blueprintRevision}-next',
      nodes: List.unmodifiable(nodes),
      createdAt: before.createdAt,
      updatedAt: before.updatedAt + _fakeMinuteMs,
    );
    _templates[index] = updated;
    return updated;
  }

  @override
  Future<TemplateDto> replaceTemplateBlueprint({
    required String templateId,
    required String taskId,
  }) async {
    final before = _templates.firstWhere((value) => value.id == templateId);
    final task = _tasks.firstWhere((value) => value.id == taskId);
    final index = _templates.indexOf(before);
    final updated = TemplateDto(
      id: before.id,
      name: before.name,
      defaultListId: before.defaultListId,
      blueprintRevision: '${before.blueprintRevision}-next',
      nodes: [
        TaskBlueprintNodeDto(
          nodeKey: task.id,
          siblingOrder: 0,
          title: task.title,
          note: task.note,
          priority: task.priority,
          estimatedMinutes: task.estimatedMinutes,
        ),
      ],
      createdAt: before.createdAt,
      updatedAt: before.updatedAt + _fakeMinuteMs,
    );
    _templates[index] = updated;
    return updated;
  }

  @override
  Future<List<TaskDto>> instantiateTemplate({
    required String templateId,
  }) async {
    final template = _templates.firstWhere((value) => value.id == templateId);
    final listId = template.defaultListId ?? _lists.first.id;
    final created = <TaskDto>[];
    for (final node in template.nodes) {
      final now = _fakeTimestamp(6000 + _taskSeq);
      final task = TaskDto(
        id: 'task-${_taskSeq++}',
        listId: listId,
        title: node.title,
        note: node.note,
        status: 'todo',
        priority: node.priority,
        estimatedMinutes: node.estimatedMinutes,
        sortOrder: 'template-${node.siblingOrder}',
        createdAt: now,
        updatedAt: now,
      );
      _tasks.add(task);
      created.add(task);
    }
    return created;
  }

  @override
  Future<TaskSeriesDto> createTaskSeriesFromTemplate({
    required String templateId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) async {
    final template = _templates.firstWhere((value) => value.id == templateId);
    final now = _fakeTimestamp(7000 + _scheduleSeq);
    final series = TaskSeriesDto(
      id: 'series-${_scheduleSeq++}',
      targetListId: template.defaultListId,
      nodes: List.unmodifiable(template.nodes),
      rrule: rrule,
      startsAt: startsAt,
      timeZone: timeZone,
      nextRunAt: startsAt,
      enabled: true,
      configRevision: 'schedule-revision-$now',
      createdAt: now,
      updatedAt: now,
    );
    _series.add(series);
    return series;
  }

  @override
  Future<TaskSeriesDto> createTaskSeriesFromTask({
    required String taskId,
    String? targetListId,
    required String rrule,
    required int startsAt,
    required String timeZone,
  }) async {
    final task = _tasks.firstWhere((value) => value.id == taskId);
    final now = _fakeTimestamp(7000 + _scheduleSeq);
    final series = TaskSeriesDto(
      id: 'series-${_scheduleSeq++}',
      targetListId: targetListId ?? task.listId,
      nodes: [
        TaskBlueprintNodeDto(
          nodeKey: task.id,
          siblingOrder: 0,
          title: task.title,
          note: task.note,
          priority: task.priority,
          estimatedMinutes: task.estimatedMinutes,
        ),
      ],
      rrule: rrule,
      startsAt: startsAt,
      timeZone: timeZone,
      nextRunAt: startsAt,
      enabled: true,
      configRevision: 'series-revision-$now',
      createdAt: now,
      updatedAt: now,
    );
    _series.add(series);
    return series;
  }

  @override
  Future<TaskSeriesDto> updateTaskSeries({
    required String seriesId,
    String? targetListId,
    required List<TaskBlueprintNodeDto> nodes,
    required String rrule,
    required int startsAt,
    required String timeZone,
    required bool enabled,
  }) async {
    final index = _series.indexWhere((value) => value.id == seriesId);
    final before = _series[index];
    final updated = TaskSeriesDto(
      id: before.id,
      targetListId: targetListId,
      nodes: nodes,
      rrule: rrule,
      startsAt: startsAt,
      timeZone: timeZone,
      nextRunAt: startsAt,
      enabled: enabled,
      configRevision: '${before.configRevision}-next',
      createdAt: before.createdAt,
      updatedAt: before.updatedAt + _fakeMinuteMs,
    );
    _series[index] = updated;
    return updated;
  }

  @override
  Future<void> deleteTaskSeries({required String seriesId}) async {
    _series.removeWhere((value) => value.id == seriesId);
  }

  @override
  Future<void> deleteTemplate({required String templateId}) async {
    _templates.removeWhere((value) => value.id == templateId);
  }

  @override
  Future<SettlementSummaryDto> settleDueSeries({required int atMs}) async =>
      const SettlementSummaryDto(
        generatedOccurrences: 0,
        generatedTasks: 0,
        hasMore: false,
        outboxChanged: false,
      );

  @override
  Future<StreakDto> getTaskSeriesStreak({
    required String seriesId,
    required int atMs,
  }) async => const StreakDto(current: 0, finalized: false);

  @override
  Future<ListDto> renameList({
    required String listId,
    required String name,
  }) async {
    if (name.trim().isEmpty) {
      throw Exception('list name must not be empty');
    }
    final index = _lists.indexWhere((list) => list.id == listId);
    final list = _lists[index];
    final updated = ListDto(
      id: list.id,
      name: name,
      color: list.color,
      icon: list.icon,
      sortOrder: list.sortOrder,
      isDefault: list.isDefault,
      archivedAt: list.archivedAt,
      createdAt: list.createdAt,
      updatedAt: list.updatedAt + _fakeMinuteMs,
    );
    _lists[index] = updated;
    return updated;
  }

  @override
  Future<ListDto> archiveList({required String listId}) async {
    final index = _lists.indexWhere((list) => list.id == listId);
    final list = _lists[index];
    if (list.archivedAt == null && list.isDefault) {
      throw Exception('default list cannot be archived');
    }
    if (list.archivedAt != null) {
      return list;
    }
    final updatedAt = list.updatedAt + _fakeMinuteMs;
    final updated = _copyList(
      list,
      archivedAt: updatedAt,
      updatedAt: updatedAt,
    );
    _lists[index] = updated;
    return updated;
  }

  @override
  Future<ListDto> unarchiveList({required String listId}) async {
    final index = _lists.indexWhere((list) => list.id == listId);
    final list = _lists[index];
    if (list.archivedAt == null) {
      return list;
    }
    final updated = _copyList(
      list,
      clearArchivedAt: true,
      updatedAt: list.updatedAt + _fakeMinuteMs,
    );
    _lists[index] = updated;
    return updated;
  }

  @override
  Future<TaskDto> createTask({
    required String listId,
    required String title,
    String? parentTaskId,
    Object? due,
    String note = '',
    int priority = 0,
    int? scheduledAt,
    int? estimatedMinutes,
  }) async {
    _validateTaskPlanningFields(
      priority: priority,
      estimatedMinutes: estimatedMinutes,
    );
    final taskSeq = _taskSeq++;
    final siblings =
        _tasks
            .where(
              (task) =>
                  task.listId == listId && task.parentTaskId == parentTaskId,
            )
            .toList()
          ..sort(_compareTasks);
    final sortOrder = _fractionalIndexBetween(
      siblings.isEmpty ? null : siblings.last.sortOrder,
      null,
    );
    final task = TaskDto(
      id: 'task-$taskSeq',
      listId: listId,
      parentTaskId: parentTaskId,
      title: title,
      note: note,
      status: 'todo',
      priority: priority,
      due: _normalizeFakeDue(due),
      scheduledAt: scheduledAt,
      estimatedMinutes: estimatedMinutes,
      sortOrder: sortOrder,
      createdAt: _fakeTimestamp(100 + taskSeq),
      updatedAt: _fakeTimestamp(100 + taskSeq),
    );
    _tasks.add(task);
    return task;
  }

  @override
  Future<List<TaskDto>> getTasks({required String listId}) async {
    final tasks = _tasks.where((task) => task.listId == listId).toList();
    tasks.sort(_compareTasks);
    return tasks;
  }

  @override
  Future<ActiveTimerSessionDto?> getActiveTimerSession() async {
    return _activeTimerSession;
  }

  @override
  Future<ActiveTimerStartOutcomeDto> startActiveTimerSession({
    required ActiveTimerSessionDto session,
  }) async {
    _validateFakeActiveTimer(session);
    final current = _activeTimerSession;
    if (current != null) {
      return ActiveTimerStartOutcomeDto.conflict;
    }
    _activeTimerSession = session;
    return ActiveTimerStartOutcomeDto.started;
  }

  @override
  Future<void> updateActiveTimerSession({
    required ActiveTimerSessionDto session,
  }) async {
    final current = _activeTimerSession;
    if (current == null || current.sessionId != session.sessionId) {
      throw Exception('active timer session does not match');
    }
    _validateFakeActiveTimer(session);
    if (current.taskId != session.taskId ||
        current.mode != session.mode ||
        current.phase != session.phase ||
        current.startedAt != session.startedAt ||
        session.accumulatedActiveMs < current.accumulatedActiveMs ||
        (current.targetDurationMs != null &&
            (session.targetDurationMs == null ||
                session.targetDurationMs! < current.targetDurationMs!))) {
      throw Exception('invalid active timer update');
    }
    final progressChanged =
        session.accumulatedActiveMs != current.accumulatedActiveMs;
    if (progressChanged &&
        !(current.state == TimerRunStateDto.running &&
            session.state == TimerRunStateDto.paused)) {
      throw Exception('invalid active timer progress');
    }
    _activeTimerSession = session;
  }

  @override
  Future<DateTime> pomodoroTargetReachedAt({
    required ActiveTimerSessionDto session,
  }) async {
    final target = session.targetDurationMs;
    final resumed = session.lastResumedAt;
    if (session.mode != TimerModeDto.pomodoro ||
        session.state != TimerRunStateDto.running ||
        target == null ||
        resumed == null) {
      throw Exception('active timer has no running target');
    }
    final remaining = target - session.accumulatedActiveMs;
    if (remaining <= 0) {
      throw Exception('timer target has already been reached');
    }
    return resumed.add(Duration(milliseconds: remaining));
  }

  @override
  Future<bool> discardActiveTimerSession({
    required String expectedSessionId,
  }) async {
    if (_activeTimerSession?.sessionId != expectedSessionId) {
      return false;
    }
    _activeTimerSession = null;
    return true;
  }

  @override
  Future<bool> finishActiveTimerSession({
    required CompletedTimerSessionDto session,
  }) async {
    final active = _activeTimerSession;
    if (active == null ||
        active.sessionId != session.id ||
        active.taskId != session.taskId ||
        active.mode != session.mode ||
        active.phase != TimerPhaseDto.work) {
      throw Exception('completed timer does not match active work');
    }
    final runningDelta = active.lastResumedAt == null
        ? 0
        : session.endedAt
              .difference(active.lastResumedAt!)
              .inMilliseconds
              .clamp(0, 7 * _fakeDayMs);
    final expectedDuration = active.state == TimerRunStateDto.paused
        ? active.accumulatedActiveMs
        : active.accumulatedActiveMs + runningDelta;
    if (session.activeDurationMs != expectedDuration ||
        session.activeDurationMs <= 0 ||
        session.activeDurationMs > 7 * _fakeDayMs ||
        session.endedAt.isAfter(
          active.startedAt.add(const Duration(days: 7)),
        )) {
      throw Exception('completed timer duration does not match active work');
    }
    final existing = _completedTimerSessions
        .where((candidate) => candidate.id == session.id)
        .firstOrNull;
    if (existing != null && existing != session) {
      throw Exception('immutable timer session conflict');
    }
    if (existing == null) {
      _completedTimerSessions.add(session);
    }
    _activeTimerSession = null;
    return existing == null;
  }

  @override
  Future<List<CompletedTimerSessionDto>> getCompletedTimerSessions({
    required String taskId,
  }) async {
    return List.unmodifiable(
      _completedTimerSessions.where((session) => session.taskId == taskId),
    );
  }

  void _validateFakeActiveTimer(ActiveTimerSessionDto session) {
    final isWork = session.phase == TimerPhaseDto.work;
    if (isWork != (session.taskId != null) ||
        (session.taskId != null &&
            !_tasks.any((task) => task.id == session.taskId))) {
      throw Exception('timer task/phase mismatch');
    }
    if (session.mode == TimerModeDto.stopwatch &&
        (session.phase != TimerPhaseDto.work ||
            session.targetDurationMs != null)) {
      throw Exception('invalid Stopwatch phase or target');
    }
    if (session.mode == TimerModeDto.pomodoro &&
        (session.targetDurationMs == null ||
            session.targetDurationMs! <= 0 ||
            session.targetDurationMs! > 7 * _fakeDayMs)) {
      throw Exception('invalid Pomodoro target');
    }
    if (session.accumulatedActiveMs < 0 ||
        session.accumulatedActiveMs > 7 * _fakeDayMs ||
        (session.state == TimerRunStateDto.running) !=
            (session.lastResumedAt != null)) {
      throw Exception('invalid timer duration or state');
    }
  }

  @override
  Future<List<TaskDto>> searchTasks({required String query}) async {
    final terms = query
        .trim()
        .toLowerCase()
        .split(RegExp(r'\s+'))
        .where((term) => term.isNotEmpty)
        .toList(growable: false);
    if (terms.isEmpty) {
      return const [];
    }
    return _tasks
        .where((task) {
          if (task.deletedAt != null) {
            return false;
          }
          final tokens = '${task.title} ${task.note}'
              .toLowerCase()
              .split(RegExp(r'\s+'))
              .where((token) => token.isNotEmpty)
              .toList(growable: false);
          return terms.every(
            (term) => tokens.any((token) => token.startsWith(term)),
          );
        })
        .toList(growable: false);
  }

  void setScheduledAtForTest({
    required String taskId,
    required int scheduledAt,
  }) {
    final index = _tasks.indexWhere((task) => task.id == taskId);
    _tasks[index] = _tasks[index]._copyWith(scheduledAt: scheduledAt);
  }

  @override
  Future<List<HomeTaskDto>> getHomeTasks({
    required int todayStartMs,
    required int tomorrowStartMs,
  }) async {
    final activeListById = {
      for (final list in _lists)
        if (list.archivedAt == null) list.id: list,
    };
    final homeTargetIds = <String>{};
    for (final task in _tasks) {
      if (!activeListById.containsKey(task.listId)) {
        continue;
      }
      if (task.status == 'todo' || task.status == 'in_progress') {
        final scheduledToday =
            task.scheduledAt != null &&
            task.scheduledAt! >= todayStartMs &&
            task.scheduledAt! < tomorrowStartMs;
        if (task.due != null || scheduledToday) {
          homeTargetIds.add(task.id);
        }
      } else if (task.status == 'done' || task.status == 'wont_do') {
        final completedAt = task.completedAt;
        if (completedAt != null &&
            completedAt >= todayStartMs &&
            completedAt < tomorrowStartMs) {
          homeTargetIds.add(task.id);
        }
      }
    }
    final childrenByParent = <String, List<TaskDto>>{};
    for (final task in _tasks) {
      final parentId = task.parentTaskId;
      if (parentId == null) {
        continue;
      }
      childrenByParent.putIfAbsent(parentId, () => <TaskDto>[]).add(task);
    }
    final homeScopeIds = <String>{};
    void includeSubtree(String taskId) {
      if (!homeScopeIds.add(taskId)) {
        return;
      }
      for (final child in childrenByParent[taskId] ?? const <TaskDto>[]) {
        includeSubtree(child.id);
      }
    }

    for (final taskId in homeTargetIds) {
      includeSubtree(taskId);
    }
    final taskById = {for (final task in _tasks) task.id: task};
    void includeAncestors(String taskId) {
      final task = taskById[taskId];
      if (task == null) {
        return;
      }
      final parentId = task.parentTaskId;
      if (parentId == null || !homeScopeIds.add(parentId)) {
        return;
      }
      includeAncestors(parentId);
    }

    for (final taskId in homeTargetIds) {
      includeAncestors(taskId);
    }

    final homeTasks = _tasks
        .where(
          (task) =>
              homeScopeIds.contains(task.id) &&
              activeListById.containsKey(task.listId),
        )
        .map(
          (task) => HomeTaskDto(
            task: task,
            listName: activeListById[task.listId]!.name,
            isHomeTarget: homeTargetIds.contains(task.id),
          ),
        )
        .toList();
    homeTasks.sort(_compareHomeTaskEntries);
    return List.unmodifiable(homeTasks);
  }

  @override
  Future<List<CalendarOccurrenceDto>> getCalendarOccurrences({
    required CalendarRangeInput range,
  }) async {
    final occurrences = <CalendarOccurrenceDto>[];
    final startAt = range.startAt.toUtc();
    final endAt = range.endAt.toUtc();
    final listsById = {for (final list in _lists) list.id: list};

    for (final task in _tasks) {
      if (task.deletedAt != null) {
        continue;
      }
      final list = listsById[task.listId];
      if (list == null) {
        continue;
      }
      final isOpen = task.status == 'todo' || task.status == 'in_progress';
      if (isOpen) {
        switch (task.due) {
          case TaskDueDto_Date(:final dueOn)
              when dueOn.compareTo(range.startOn) >= 0 &&
                  dueOn.compareTo(range.endOn) < 0:
            occurrences.add(
              CalendarOccurrenceDto(
                task: task,
                listName: list.name,
                listArchived: list.archivedAt != null,
                kind: CalendarOccurrenceKindDto.dateDue(dueOn: dueOn),
              ),
            );
          case TaskDueDto_DateTime(:final dueAt, :final timeZone)
              when !dueAt.toUtc().isBefore(startAt) &&
                  dueAt.toUtc().isBefore(endAt):
            occurrences.add(
              CalendarOccurrenceDto(
                task: task,
                listName: list.name,
                listArchived: list.archivedAt != null,
                kind: CalendarOccurrenceKindDto.dateTimeDue(
                  dueAt: dueAt,
                  timeZone: timeZone,
                ),
              ),
            );
          default:
            break;
        }
        final scheduledAt = task.scheduledAt;
        if (scheduledAt != null) {
          final instant = DateTime.fromMillisecondsSinceEpoch(
            scheduledAt,
            isUtc: true,
          );
          if (!instant.isBefore(startAt) && instant.isBefore(endAt)) {
            occurrences.add(
              CalendarOccurrenceDto(
                task: task,
                listName: list.name,
                listArchived: list.archivedAt != null,
                kind: CalendarOccurrenceKindDto.scheduled(scheduledAt: instant),
              ),
            );
          }
        }
        continue;
      }

      if (task.status == 'done' || task.status == 'wont_do') {
        final completedAt = task.completedAt;
        if (completedAt == null) {
          continue;
        }
        final instant = DateTime.fromMillisecondsSinceEpoch(
          completedAt,
          isUtc: true,
        );
        if (!instant.isBefore(startAt) && instant.isBefore(endAt)) {
          occurrences.add(
            CalendarOccurrenceDto(
              task: task,
              listName: list.name,
              listArchived: list.archivedAt != null,
              kind: CalendarOccurrenceKindDto.completed(completedAt: instant),
            ),
          );
        }
      }
    }
    return List.unmodifiable(occurrences);
  }

  void setTaskCalendarStateForTest({
    required String taskId,
    String? status,
    int? completedAt,
    int? deletedAt,
  }) {
    final index = _tasks.indexWhere((task) => task.id == taskId);
    _tasks[index] = _tasks[index]._copyWith(
      status: status,
      completedAt: completedAt,
      deletedAt: deletedAt,
    );
  }

  @override
  Future<int> countTasksInList({required String listId}) async {
    return _tasks.where((task) => task.listId == listId).length;
  }

  @override
  Future<TaskDto> updateTask({
    required String taskId,
    required String title,
    required String note,
    required int priority,
    Object? due,
    int? scheduledAt,
    int? estimatedMinutes,
  }) async {
    updateTaskCalls.add(taskId);
    if (title.trim().isEmpty) {
      throw Exception('task title must not be empty');
    }
    _validateTaskPlanningFields(
      priority: priority,
      estimatedMinutes: estimatedMinutes,
    );
    final index = _tasks.indexWhere((task) => task.id == taskId);
    final task = _tasks[index];
    final updatedAt = task.updatedAt + _fakeMinuteMs;
    final updated = TaskDto(
      id: task.id,
      listId: task.listId,
      parentTaskId: task.parentTaskId,
      title: title,
      note: note,
      status: task.status,
      priority: priority,
      due: _normalizeFakeDue(due),
      scheduledAt: scheduledAt,
      estimatedMinutes: estimatedMinutes,
      sortOrder: task.sortOrder,
      completedAt: task.completedAt,
      closedReason: task.closedReason,
      deletedAt: task.deletedAt,
      assignee: task.assignee,
      createdAt: task.createdAt,
      updatedAt: updatedAt,
    );
    _tasks[index] = updated;
    _recordUndo(operationType: 'edit', before: task, after: updated);
    return updated;
  }

  @override
  Future<TaskDto> setTaskStatus({
    required String taskId,
    required String status,
    String? closedReason,
  }) async {
    final index = _tasks.indexWhere((task) => task.id == taskId);
    final before = _tasks[index];
    if (!_canTransition(before.status, status)) {
      throw Exception('invalid task status transition');
    }
    final updatedAt = before.updatedAt + _fakeMinuteMs;
    final isClosed = status == 'done' || status == 'wont_do';
    final completedAt = isClosed ? DateTime.now().millisecondsSinceEpoch : null;
    final updated = before._copyWith(
      status: status,
      completedAt: completedAt,
      closedReason: status == 'wont_do' ? closedReason : null,
      clearCompletedAt: !isClosed,
      clearClosedReason: status != 'wont_do',
      updatedAt: updatedAt,
    );
    _tasks[index] = updated;
    if (status == 'done' || status == 'wont_do') {
      _recordUndo(operationType: 'complete', before: before, after: updated);
    }
    return updated;
  }

  @override
  Future<int> countTaskDescendants({required String taskId}) async {
    return _descendantIds(taskId).length;
  }

  @override
  Future<void> deleteTask({required String taskId}) async {
    final ids = {taskId, ..._descendantIds(taskId)};
    _tasks.removeWhere((task) => ids.contains(task.id));
    _reminders.removeWhere((reminder) => ids.contains(reminder.taskId));
    _undoEntries.removeWhere((entry) => ids.contains(entry.taskId));
    _completedTimerSessions.removeWhere(
      (session) => ids.contains(session.taskId),
    );
    if (ids.contains(_activeTimerSession?.taskId)) {
      _activeTimerSession = null;
    }
  }

  @override
  Future<void> deleteList({required String listId}) async {
    final list = _lists.singleWhere((candidate) => candidate.id == listId);
    if (list.isDefault) {
      throw Exception('default list cannot be deleted');
    }
    final defaultList = _lists.singleWhere((candidate) => candidate.isDefault);
    for (var index = 0; index < _tasks.length; index++) {
      final task = _tasks[index];
      if (task.listId == listId) {
        _tasks[index] = task._copyWith(
          listId: defaultList.id,
          updatedAt: task.updatedAt + _fakeMinuteMs,
        );
      }
    }
    _lists.removeWhere((candidate) => candidate.id == listId);
  }

  @override
  Future<TaskDto> reorderTask({
    required String taskId,
    String? previousTaskId,
    String? nextTaskId,
  }) async {
    reorderCalls.add(
      FakeReorderCall(
        taskId: taskId,
        previousTaskId: previousTaskId,
        nextTaskId: nextTaskId,
      ),
    );
    if (previousTaskId == taskId || nextTaskId == taskId) {
      throw Exception('task cannot be reordered relative to itself');
    }
    if (previousTaskId != null && previousTaskId == nextTaskId) {
      throw Exception('previous and next task must be different');
    }

    final index = _tasks.indexWhere((task) => task.id == taskId);
    final task = _tasks[index];
    final previous = previousTaskId == null
        ? null
        : _reorderBoundary(previousTaskId, task);
    final next = nextTaskId == null ? null : _reorderBoundary(nextTaskId, task);
    final updatedAt = task.updatedAt + _fakeMinuteMs;
    final updated = task._copyWith(
      sortOrder: _fractionalIndexBetween(previous?.sortOrder, next?.sortOrder),
      updatedAt: updatedAt,
    );
    _tasks[index] = updated;
    return updated;
  }

  @override
  Future<TaskUndoDto?> getLatestTaskUndo() async {
    final available = _undoEntries
        .where((entry) => !entry.consumed && entry.operationType != 'delete')
        .toList(growable: false);
    if (available.isEmpty) {
      return null;
    }
    available.sort((a, b) => b.createdAt.compareTo(a.createdAt));
    return available.first.dto;
  }

  @override
  Future<TaskDto> undoTaskOperation({required String undoId}) async {
    final entry = _undoEntries.singleWhere(
      (candidate) => candidate.id == undoId,
    );
    if (entry.consumed) {
      throw Exception('undo entry already used');
    }
    final index = _tasks.indexWhere((task) => task.id == entry.taskId);
    if (index < 0) {
      throw Exception('record not found');
    }
    final current = _tasks[index];
    if (current.updatedAt != entry.afterUpdatedAt ||
        current.deletedAt != entry.afterDeletedAt ||
        current.completedAt != entry.afterCompletedAt) {
      throw Exception('task changed after undo was created');
    }
    entry.consumed = true;
    _tasks[index] = entry.before;
    return entry.before;
  }

  @override
  Future<String?> getFrontendSetting({
    required FrontendSettingKeyDto key,
  }) async {
    return _settings[key];
  }

  @override
  Future<void> setFrontendSetting({
    required FrontendSettingKeyDto key,
    required String value,
  }) async {
    _settings[key] = value;
  }

  @override
  Future<ReminderDto> createTaskReminder({
    required String taskId,
    required int remindAt,
  }) async {
    final task = _tasks.where((task) => task.id == taskId).firstOrNull;
    if (task == null) {
      throw Exception('record not found');
    }
    if (task.status == 'done' || task.status == 'wont_do') {
      throw Exception('closed tasks cannot schedule reminders');
    }
    if (remindAt <= DateTime.now().millisecondsSinceEpoch) {
      throw Exception('reminder time must be in the future');
    }
    final taskReminders = _reminders.where(
      (reminder) => reminder.taskId == taskId,
    );
    if (taskReminders.length >= 5) {
      throw Exception('a task can have at most 5 reminders');
    }
    if (taskReminders.any((reminder) => reminder.remindAt == remindAt)) {
      throw Exception('a reminder already exists at that time');
    }
    final reminderSeq = _reminderSeq++;
    final reminder = ReminderDto(
      id: 'reminder-$reminderSeq',
      taskId: taskId,
      remindAt: remindAt,
      createdAt: _fakeTimestamp(2000 + reminderSeq),
    );
    _reminders.add(reminder);
    return reminder;
  }

  @override
  Future<ReminderDto> updateReminder({
    required String reminderId,
    required int remindAt,
  }) async {
    final index = _reminders.indexWhere(
      (reminder) => reminder.id == reminderId,
    );
    if (index < 0) {
      throw Exception('record not found');
    }
    if (remindAt <= DateTime.now().millisecondsSinceEpoch) {
      throw Exception('reminder time must be in the future');
    }
    final current = _reminders[index];
    if (_reminders.any(
      (reminder) =>
          reminder.id != reminderId &&
          reminder.taskId == current.taskId &&
          reminder.remindAt == remindAt,
    )) {
      throw Exception('a reminder already exists at that time');
    }
    final updated = ReminderDto(
      id: current.id,
      taskId: current.taskId,
      remindAt: remindAt,
      createdAt: current.createdAt,
    );
    _reminders[index] = updated;
    return updated;
  }

  @override
  Future<ReminderDto> deleteReminder({required String reminderId}) async {
    final index = _reminders.indexWhere(
      (reminder) => reminder.id == reminderId,
    );
    if (index < 0) {
      throw Exception('record not found');
    }
    return _reminders.removeAt(index);
  }

  @override
  Future<List<ReminderDto>> clearTaskReminders({required String taskId}) async {
    final removed = _reminders
        .where((reminder) => reminder.taskId == taskId)
        .toList(growable: false);
    _reminders.removeWhere((reminder) => reminder.taskId == taskId);
    return List.unmodifiable(removed);
  }

  @override
  Future<List<ReminderDto>> getTaskReminders({required String taskId}) async {
    final reminders = _reminders
        .where((reminder) => reminder.taskId == taskId)
        .toList(growable: false);
    reminders.sort(_compareReminders);
    return List.unmodifiable(reminders);
  }

  @override
  Future<List<ReminderDto>> getTaskSubtreeReminders({
    required String taskId,
  }) async {
    final ids = {taskId, ..._descendantIds(taskId)};
    final reminders = _reminders
        .where((reminder) => ids.contains(reminder.taskId))
        .toList(growable: false);
    reminders.sort(_compareReminders);
    return List.unmodifiable(reminders);
  }

  @override
  Future<List<ReminderDto>> getListReminders({required String listId}) async {
    final taskIds = _tasks
        .where((task) => task.listId == listId)
        .map((task) => task.id)
        .toSet();
    final reminders = _reminders
        .where((reminder) => taskIds.contains(reminder.taskId))
        .toList(growable: false);
    reminders.sort(_compareReminders);
    return List.unmodifiable(reminders);
  }

  @override
  Future<List<ReminderDto>> listPendingReminders({required int nowMs}) async {
    final openTaskIds = _tasks
        .where((task) => task.status == 'todo' || task.status == 'in_progress')
        .map((task) => task.id)
        .toSet();
    final reminders = _reminders
        .where(
          (reminder) =>
              openTaskIds.contains(reminder.taskId) &&
              _effectiveReminderAt(reminder) > nowMs,
        )
        .toList(growable: false);
    reminders.sort(_compareReminders);
    return List.unmodifiable(reminders);
  }

  @override
  Future<ReminderDto> snoozeReminder({
    required String reminderId,
    required int snoozedUntil,
  }) async {
    final index = _reminders.indexWhere(
      (reminder) => reminder.id == reminderId,
    );
    if (index < 0) {
      throw Exception('record not found');
    }
    final current = _reminders[index];
    final task = _tasks.singleWhere((task) => task.id == current.taskId);
    if (task.status == 'done' || task.status == 'wont_do') {
      throw Exception('closed tasks cannot snooze reminders');
    }
    final updated = ReminderDto(
      id: current.id,
      taskId: current.taskId,
      remindAt: current.remindAt,
      snoozedUntil: snoozedUntil,
      createdAt: current.createdAt,
    );
    _reminders[index] = updated;
    return updated;
  }

  TaskDto _reorderBoundary(String boundaryId, TaskDto task) {
    final boundary = _tasks.singleWhere(
      (candidate) => candidate.id == boundaryId,
    );
    if (boundary.listId != task.listId) {
      throw Exception('reorder boundary belongs to a different list');
    }
    if (boundary.parentTaskId != task.parentTaskId) {
      throw Exception('reorder boundary belongs to a different parent');
    }
    return boundary;
  }

  void _recordUndo({
    required String operationType,
    required TaskDto before,
    required TaskDto after,
  }) {
    final undoSeq = _undoSeq++;
    final id = 'undo-$undoSeq';
    final createdAt = _fakeTimestamp(1000 + undoSeq);
    _undoEntries.add(
      FakeTaskUndoEntry(
        id: id,
        operationType: operationType,
        taskId: before.id,
        before: before,
        afterUpdatedAt: after.updatedAt,
        afterDeletedAt: after.deletedAt,
        afterCompletedAt: after.completedAt,
        createdAt: createdAt,
        dto: TaskUndoDto(
          id: id,
          operationType: operationType,
          taskId: before.id,
          listId: before.listId,
          taskTitle: before.title,
          createdAt: createdAt,
        ),
      ),
    );
  }

  Set<String> _descendantIds(String taskId) {
    final descendants = <String>{};
    var frontier = <String>{taskId};
    while (frontier.isNotEmpty) {
      final next = _tasks
          .where((task) => frontier.contains(task.parentTaskId))
          .map((task) => task.id)
          .where(descendants.add)
          .toSet();
      frontier = next;
    }
    return descendants;
  }

  AccountSessionStateDto _newAccountSession(
    String email, {
    bool recoveryPending = false,
  }) {
    final accountSeq = _accountSeq++;
    return AccountSessionStateDto(
      loggedIn: true,
      email: email,
      userId: 'user-$accountSeq',
      tenantId: 'tenant-$accountSeq',
      deviceId: 'device-$accountSeq',
      recoveryPending: recoveryPending,
    );
  }
}

class FakeLargeSeedSummary {
  const FakeLargeSeedSummary({
    required this.listCount,
    required this.taskCount,
    required this.dueTaskCount,
    required this.closedTaskCount,
    required this.defaultListId,
  });

  final int listCount;
  final int taskCount;
  final int dueTaskCount;
  final int closedTaskCount;
  final String defaultListId;
}

bool _canTransition(String current, String next) {
  if (current == next) {
    return false;
  }
  return switch ((current, next)) {
    ('todo', 'in_progress') ||
    ('todo', 'done') ||
    ('todo', 'wont_do') ||
    ('in_progress', 'todo') ||
    ('in_progress', 'done') ||
    ('in_progress', 'wont_do') ||
    ('done', 'todo') ||
    ('wont_do', 'todo') => true,
    _ => false,
  };
}

final int _fakeClockBaseMs = DateTime.utc(2026, 7, 1, 9).millisecondsSinceEpoch;

const int _fakeMinuteMs = Duration.millisecondsPerMinute;
const int _fakeTenMinuteMs = 10 * Duration.millisecondsPerMinute;
const int _fakeHourMs = Duration.millisecondsPerHour;
const int _fakeDayMs = Duration.millisecondsPerDay;

int _fakeTimestamp(int sequence) =>
    _fakeClockBaseMs + (sequence * _fakeMinuteMs);

/// A recorded undo entry for [FakeBridgeService].
///
/// Public so it can be referenced from other test support code if needed;
/// mirrors the shape of the real undo log without touching storage.
class FakeTaskUndoEntry {
  FakeTaskUndoEntry({
    required this.id,
    required this.operationType,
    required this.taskId,
    required this.before,
    required this.afterUpdatedAt,
    required this.afterDeletedAt,
    required this.afterCompletedAt,
    required this.createdAt,
    required this.dto,
  });

  final String id;
  final String operationType;
  final String taskId;
  final TaskDto before;
  final int afterUpdatedAt;
  final int? afterDeletedAt;
  final int? afterCompletedAt;
  final int createdAt;
  final TaskUndoDto dto;
  bool consumed = false;
}

class FakeReorderCall {
  const FakeReorderCall({
    required this.taskId,
    required this.previousTaskId,
    required this.nextTaskId,
  });

  final String taskId;
  final String? previousTaskId;
  final String? nextTaskId;
}

extension _TaskDtoCopy on TaskDto {
  TaskDto _copyWith({
    String? listId,
    String? title,
    String? note,
    String? status,
    int? priority,
    Object? due = _unchangedTaskDue,
    int? completedAt,
    String? closedReason,
    int? deletedAt,
    int? scheduledAt,
    String? sortOrder,
    int? updatedAt,
    bool clearCompletedAt = false,
    bool clearClosedReason = false,
  }) {
    return TaskDto(
      id: id,
      listId: listId ?? this.listId,
      parentTaskId: parentTaskId,
      title: title ?? this.title,
      note: note ?? this.note,
      status: status ?? this.status,
      priority: priority ?? this.priority,
      due: identical(due, _unchangedTaskDue) ? this.due : due as TaskDueDto?,
      scheduledAt: scheduledAt ?? this.scheduledAt,
      estimatedMinutes: estimatedMinutes,
      sortOrder: sortOrder ?? this.sortOrder,
      completedAt: clearCompletedAt ? null : completedAt ?? this.completedAt,
      closedReason: clearClosedReason
          ? null
          : closedReason ?? this.closedReason,
      deletedAt: deletedAt ?? this.deletedAt,
      assignee: assignee,
      createdAt: createdAt,
      updatedAt: updatedAt ?? this.updatedAt,
    );
  }
}

int _compareTasks(TaskDto a, TaskDto b) {
  final sortOrder = a.sortOrder.compareTo(b.sortOrder);
  if (sortOrder != 0) {
    return sortOrder;
  }
  return a.id.compareTo(b.id);
}

int _compareHomeTaskEntries(HomeTaskDto a, HomeTaskDto b) {
  final due = compareTaskDue(a.task.due, b.task.due);
  if (due != 0) {
    return due;
  }
  return _compareTasks(a.task, b.task);
}

const _unchangedTaskDue = Object();

TaskDueDto? _normalizeFakeDue(Object? value) => switch (value) {
  null => null,
  TaskDueDto due => due,
  TaskDueInput due => taskDueDto(due),
  _ => throw ArgumentError.value(value, 'due'),
};

void _validateTaskPlanningFields({
  required int priority,
  required int? estimatedMinutes,
}) {
  if (priority < 0 || priority > 3) {
    throw Exception('task priority must be between 0 and 3');
  }
  if (estimatedMinutes != null &&
      (estimatedMinutes <= 0 || estimatedMinutes % 5 != 0)) {
    throw Exception('estimated minutes must be a positive multiple of 5');
  }
}

int _compareReminders(ReminderDto a, ReminderDto b) {
  final effectiveAt = _effectiveReminderAt(
    a,
  ).compareTo(_effectiveReminderAt(b));
  if (effectiveAt != 0) {
    return effectiveAt;
  }
  final createdAt = a.createdAt.compareTo(b.createdAt);
  if (createdAt != 0) {
    return createdAt;
  }
  return a.id.compareTo(b.id);
}

int _effectiveReminderAt(ReminderDto reminder) =>
    reminder.snoozedUntil ?? reminder.remindAt;

int _compareLists(ListDto a, ListDto b) {
  final sortOrder = a.sortOrder.compareTo(b.sortOrder);
  if (sortOrder != 0) {
    return sortOrder;
  }
  return a.id.compareTo(b.id);
}

ListDto _copyList(
  ListDto list, {
  String? name,
  int? archivedAt,
  bool clearArchivedAt = false,
  int? updatedAt,
}) {
  return ListDto(
    id: list.id,
    name: name ?? list.name,
    color: list.color,
    icon: list.icon,
    sortOrder: list.sortOrder,
    isDefault: list.isDefault,
    archivedAt: clearArchivedAt ? null : archivedAt ?? list.archivedAt,
    createdAt: list.createdAt,
    updatedAt: updatedAt ?? list.updatedAt,
  );
}

SyncStatusDto _copySyncStatus(
  SyncStatusDto status, {
  bool? loggedIn,
  bool? running,
  int? lastSuccessAt,
  int? lastFailureAt,
  String? lastError,
}) {
  return SyncStatusDto(
    loggedIn: loggedIn ?? status.loggedIn,
    running: running ?? status.running,
    lastSuccessAt: lastSuccessAt ?? status.lastSuccessAt,
    lastFailureAt: lastFailureAt ?? status.lastFailureAt,
    lastError: lastError,
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

const _sortAlphabet =
    '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';

String _fractionalIndexBetween(String? previous, String? next) {
  if (previous != null) {
    _validateSortOrder(previous);
  }
  if (next != null) {
    _validateSortOrder(next);
  }
  if (previous != null && next != null && previous.compareTo(next) >= 0) {
    throw Exception('invalid sort order boundary');
  }

  final buffer = StringBuffer();
  var index = 0;
  while (true) {
    final previousDigit = _digitAt(previous, index, isPrevious: true);
    final nextDigit = _digitAt(next, index, isPrevious: false);
    if (nextDigit - previousDigit > 1) {
      return '${buffer.toString()}'
          '${_sortAlphabet[(previousDigit + ((nextDigit - previousDigit) ~/ 2))]}';
    }
    if (previousDigit < 0) {
      if (next != null && index + 1 < next.length) {
        return '${buffer.toString()}${_sortAlphabet[nextDigit]}';
      }
      throw Exception('sort order space is exhausted');
    }
    buffer.write(_sortAlphabet[previousDigit]);
    index += 1;
  }
}

void _validateSortOrder(String value) {
  if (value.isEmpty ||
      value.split('').any((char) => !_sortAlphabet.contains(char))) {
    throw Exception('invalid sort order');
  }
}

int _digitAt(String? value, int index, {required bool isPrevious}) {
  if (value == null) {
    return isPrevious ? -1 : _sortAlphabet.length;
  }
  if (index >= value.length) {
    return isPrevious ? -1 : _sortAlphabet.length;
  }
  return _sortAlphabet.indexOf(value[index]);
}

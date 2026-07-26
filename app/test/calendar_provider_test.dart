import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:timezone/data/latest_all.dart' as tz_data;
import 'package:timezone/timezone.dart' as tz;
import 'package:taskveil/src/core/civil_time.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/rust/api.dart';

import 'support/fake_bridge_service.dart';

void main() {
  setUpAll(tz_data.initializeTimeZones);

  test('CalendarRange uses civil midnights across 23h and 25h DST days', () {
    final location = tz.getLocation('America/New_York');
    final spring = CalendarRange.local(
      start: tz.TZDateTime(location, 2025, 3, 9),
      end: tz.TZDateTime(location, 2025, 3, 10),
    );
    final autumn = CalendarRange.local(
      start: tz.TZDateTime(location, 2025, 11, 2),
      end: tz.TZDateTime(location, 2025, 11, 3),
    );

    expect(spring.startOn, '2025-03-09');
    expect(spring.endOn, '2025-03-10');
    expect(spring.endAt.difference(spring.startAt), const Duration(hours: 23));
    expect(autumn.endAt.difference(autumn.startAt), const Duration(hours: 25));
  });

  test('shared civil day helper preserves explicit timezone and DST', () {
    final location = tz.getLocation('America/New_York');
    final springDay = tz.TZDateTime(location, 2025, 3, 9, 12);
    final autumnDay = tz.TZDateTime(location, 2025, 11, 2, 12);

    final springStart = localCivilDay(springDay);
    final springEnd = localCivilDay(springDay, dayOffset: 1);
    final autumnStart = localCivilDay(autumnDay);
    final autumnEnd = localCivilDay(autumnDay, dayOffset: 1);

    expect(springStart, isA<tz.TZDateTime>());
    expect((springStart as tz.TZDateTime).location, location);
    expect(
      (springStart.year, springStart.month, springStart.day, springStart.hour),
      (2025, 3, 9, 0),
    );
    expect(
      (springEnd.year, springEnd.month, springEnd.day, springEnd.hour),
      (2025, 3, 10, 0),
    );
    expect(springEnd.difference(springStart), const Duration(hours: 23));
    expect(autumnEnd.difference(autumnStart), const Duration(hours: 25));
  });

  test('Home ranges use civil DST, month-end, and year-end boundaries', () {
    final location = tz.getLocation('America/New_York');
    final springNow = tz.TZDateTime(location, 2025, 3, 9, 12);
    final spring = homeLocalRangesMs(now: springNow);
    final today = todayLocalRangeMs(now: springNow);
    final autumn = homeLocalRangesMs(
      now: tz.TZDateTime(location, 2025, 11, 2, 12),
    );

    expect(today.startMs, spring.todayStartMs);
    expect(today.endMs, spring.tomorrowStartMs);
    expect(
      spring.tomorrowStartMs - spring.todayStartMs,
      const Duration(hours: 23).inMilliseconds,
    );
    expect(
      autumn.tomorrowStartMs - autumn.todayStartMs,
      const Duration(hours: 25).inMilliseconds,
    );
    _expectCivilBoundaries(location, spring, const [
      (2025, 3, 9),
      (2025, 3, 10),
      (2025, 3, 11),
    ]);
    _expectCivilBoundaries(location, autumn, const [
      (2025, 11, 2),
      (2025, 11, 3),
      (2025, 11, 4),
    ]);
    _expectCivilBoundaries(
      location,
      homeLocalRangesMs(now: tz.TZDateTime(location, 2026, 1, 31, 12)),
      const [(2026, 1, 31), (2026, 2, 1), (2026, 2, 2)],
    );
    _expectCivilBoundaries(
      location,
      homeLocalRangesMs(now: tz.TZDateTime(location, 2026, 12, 31, 12)),
      const [(2026, 12, 31), (2027, 1, 1), (2027, 1, 2)],
    );
  });

  test('Home due classification excludes tomorrow DST civil midnight', () {
    final location = tz.getLocation('America/New_York');
    final range = homeLocalRangesMs(
      now: tz.TZDateTime(location, 2025, 3, 9, 12),
    );
    final dueInside = tz.TZDateTime.fromMillisecondsSinceEpoch(
      location,
      range.tomorrowStartMs - 1,
    );
    final dueAtEnd = tz.TZDateTime.fromMillisecondsSinceEpoch(
      location,
      range.tomorrowStartMs,
    );

    expect(
      localCivilDay(dueInside).millisecondsSinceEpoch < range.tomorrowStartMs,
      isTrue,
    );
    expect(
      localCivilDay(dueAtEnd).millisecondsSinceEpoch < range.tomorrowStartMs,
      isFalse,
    );
  });

  test('CalendarRange.day shares DST-safe civil boundaries', () {
    final location = tz.getLocation('America/New_York');
    final spring = CalendarRange.day(tz.TZDateTime(location, 2025, 3, 9, 12));
    final autumn = CalendarRange.day(tz.TZDateTime(location, 2025, 11, 2, 12));

    expect(spring.startOn, '2025-03-09');
    expect(spring.endOn, '2025-03-10');
    expect(spring.endAt.difference(spring.startAt), const Duration(hours: 23));
    expect(autumn.endAt.difference(autumn.startAt), const Duration(hours: 25));
  });

  test(
    'Home scheduled and completed filters use the DST half-open range',
    () async {
      final location = tz.getLocation('America/New_York');
      final range = homeLocalRangesMs(
        now: tz.TZDateTime(location, 2025, 3, 9, 12),
      );
      final fake = FakeBridgeService();
      final inbox = await fake.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final scheduledInside = await fake.createTask(
        listId: inbox.id,
        title: 'Scheduled inside',
      );
      fake.setScheduledAtForTest(
        taskId: scheduledInside.id,
        scheduledAt: range.tomorrowStartMs - 1,
      );
      final scheduledAtEnd = await fake.createTask(
        listId: inbox.id,
        title: 'Scheduled at end',
      );
      fake.setScheduledAtForTest(
        taskId: scheduledAtEnd.id,
        scheduledAt: range.tomorrowStartMs,
      );
      final completedInside = await fake.createTask(
        listId: inbox.id,
        title: 'Completed inside without planning',
      );
      fake.setTaskCalendarStateForTest(
        taskId: completedInside.id,
        status: 'done',
        completedAt: range.tomorrowStartMs - 1,
      );
      final completedAtEnd = await fake.createTask(
        listId: inbox.id,
        title: 'Completed at end without planning',
      );
      fake.setTaskCalendarStateForTest(
        taskId: completedAtEnd.id,
        status: 'wont_do',
        completedAt: range.tomorrowStartMs,
      );

      final home = await fake.getHomeTasks(
        todayStartMs: range.todayStartMs,
        tomorrowStartMs: range.tomorrowStartMs,
      );
      final ids = home.map((entry) => entry.task.id).toSet();

      expect(ids, containsAll([scheduledInside.id, completedInside.id]));
      expect(ids, isNot(contains(scheduledAtEnd.id)));
      expect(ids, isNot(contains(completedAtEnd.id)));
    },
  );

  test(
    'Calendar due scheduled and completed instants exclude the DST range end',
    () async {
      final location = tz.getLocation('America/New_York');
      final range = CalendarRange.day(tz.TZDateTime(location, 2025, 3, 9, 12));
      final inside = range.endAt.millisecondsSinceEpoch - 1;
      final end = range.endAt.millisecondsSinceEpoch;
      final fake = FakeBridgeService();
      final inbox = await fake.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final dueInside = await fake.createTask(
        listId: inbox.id,
        title: 'Due inside',
        due: dateTimeDueFromInstant(
          DateTime.fromMillisecondsSinceEpoch(inside, isUtc: true),
          timeZone: location.name,
        ),
      );
      final dueAtEnd = await fake.createTask(
        listId: inbox.id,
        title: 'Due at end',
        due: dateTimeDueFromInstant(
          DateTime.fromMillisecondsSinceEpoch(end, isUtc: true),
          timeZone: location.name,
        ),
      );
      final scheduledInside = await fake.createTask(
        listId: inbox.id,
        title: 'Scheduled inside',
        scheduledAt: inside,
      );
      final scheduledAtEnd = await fake.createTask(
        listId: inbox.id,
        title: 'Scheduled at end',
        scheduledAt: end,
      );
      final completedInside = await fake.createTask(
        listId: inbox.id,
        title: 'Completed inside',
      );
      fake.setTaskCalendarStateForTest(
        taskId: completedInside.id,
        status: 'done',
        completedAt: inside,
      );
      final completedAtEnd = await fake.createTask(
        listId: inbox.id,
        title: 'Completed at end',
      );
      fake.setTaskCalendarStateForTest(
        taskId: completedAtEnd.id,
        status: 'wont_do',
        completedAt: end,
      );

      final occurrences = await fake.getCalendarOccurrences(
        range: range.toInput(),
      );
      final ids = occurrences.map((entry) => entry.task.id).toSet();

      expect(
        ids,
        containsAll([dueInside.id, scheduledInside.id, completedInside.id]),
      );
      expect(ids, isNot(contains(dueAtEnd.id)));
      expect(ids, isNot(contains(scheduledAtEnd.id)));
      expect(ids, isNot(contains(completedAtEnd.id)));
    },
  );

  test('CalendarRange rejects UTC boundaries as viewer-local civil dates', () {
    expect(
      () => CalendarRange.local(
        start: DateTime.utc(2026, 7, 13),
        end: DateTime.utc(2026, 7, 14),
      ),
      throwsArgumentError,
    );
  });

  test(
    'calendar returns dual open occurrences and only closed outcomes',
    () async {
      final fake = FakeBridgeService();
      final inbox = await fake.createDefaultList(
        name: 'Inbox',
        sortOrder: 'a0',
      );
      final archive = await fake.createList(name: 'Archive', sortOrder: 'a1');
      await fake.archiveList(listId: archive.id);
      final day = DateTime(2026, 7, 13);
      final range = CalendarRange.day(day);
      final inside = DateTime(2026, 7, 13, 9).millisecondsSinceEpoch;
      final end = DateTime(2026, 7, 14).millisecondsSinceEpoch;

      final dual = await fake.createTask(
        listId: inbox.id,
        title: 'Dual',
        due: dateOnlyDue(day),
        scheduledAt: inside,
      );
      final inProgress = await fake.createTask(
        listId: archive.id,
        title: 'Archived active',
        due: dateTimeDueFromInstant(
          DateTime.fromMillisecondsSinceEpoch(inside),
        ),
      );
      fake.setTaskCalendarStateForTest(
        taskId: inProgress.id,
        status: 'in_progress',
      );
      final done = await fake.createTask(
        listId: inbox.id,
        title: 'Done',
        due: dateOnlyDue(day),
        scheduledAt: inside,
      );
      fake.setTaskCalendarStateForTest(
        taskId: done.id,
        status: 'done',
        completedAt: inside,
      );
      final wontDo = await fake.createTask(listId: inbox.id, title: 'Wont do');
      fake.setTaskCalendarStateForTest(
        taskId: wontDo.id,
        status: 'wont_do',
        completedAt: inside,
      );
      final deleted = await fake.createTask(
        listId: inbox.id,
        title: 'Deleted',
        due: dateOnlyDue(day),
      );
      fake.setTaskCalendarStateForTest(taskId: deleted.id, deletedAt: inside);
      await fake.createTask(
        listId: inbox.id,
        title: 'End excluded',
        scheduledAt: end,
      );

      final occurrences = await fake.getCalendarOccurrences(
        range: range.toInput(),
      );

      expect(
        occurrences.where((entry) => entry.task.id == dual.id),
        hasLength(2),
      );
      expect(
        occurrences
            .where((entry) => entry.task.id == inProgress.id)
            .single
            .listArchived,
        isTrue,
      );
      expect(
        occurrences.where((entry) => entry.task.id == done.id).single.kind,
        isA<CalendarOccurrenceKindDto_Completed>(),
      );
      expect(
        occurrences.where((entry) => entry.task.id == wontDo.id).single.kind,
        isA<CalendarOccurrenceKindDto_Completed>(),
      );
      expect(occurrences.any((entry) => entry.task.id == deleted.id), isFalse);
      expect(
        occurrences.any((entry) => entry.task.title == 'End excluded'),
        isFalse,
      );
    },
  );

  test(
    'moving due preserves scheduled and moving scheduled preserves due',
    () async {
      final fake = FakeBridgeService();
      final list = await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
      final initialDay = DateTime(2026, 7, 13);
      final initialScheduled = DateTime(
        2026,
        7,
        13,
        14,
        35,
      ).millisecondsSinceEpoch;
      final task = await fake.createTask(
        listId: list.id,
        title: 'Move independently',
        due: dateOnlyDue(initialDay),
        scheduledAt: initialScheduled,
      );
      final container = ProviderContainer(
        overrides: [bridgeServiceProvider.overrideWithValue(fake)],
      );
      addTearDown(container.dispose);
      final range = CalendarRange.day(initialDay);
      final initialOccurrences = await container.read(
        calendarOccurrencesProvider(range).future,
      );
      final dueOccurrence = initialOccurrences.singleWhere(
        (entry) => entry.kind is CalendarOccurrenceKindDto_DateDue,
      );

      await container
          .read(calendarOccurrencesProvider(range).notifier)
          .moveOccurrence(
            occurrence: dueOccurrence,
            targetDate: DateTime(2026, 7, 15),
          );
      var persisted = (await fake.getTasks(listId: list.id)).single;
      expect(taskDueCivilDate(persisted.due), '2026-07-15');
      expect(persisted.scheduledAt, initialScheduled);

      final scheduledOccurrence =
          (await container.read(
            calendarOccurrencesProvider(range).future,
          )).singleWhere(
            (entry) => entry.kind is CalendarOccurrenceKindDto_Scheduled,
          );
      await container
          .read(calendarOccurrencesProvider(range).notifier)
          .moveOccurrence(
            occurrence: scheduledOccurrence,
            targetDate: DateTime(2026, 7, 16),
          );
      persisted = (await fake.getTasks(listId: list.id)).single;
      expect(taskDueCivilDate(persisted.due), '2026-07-15');
      expect(
        persisted.scheduledAt,
        DateTime(2026, 7, 16, 14, 35).millisecondsSinceEpoch,
      );
      expect(fake.updateTaskCalls, [task.id, task.id]);
    },
  );

  test('moving completed occurrence is rejected without task update', () async {
    final fake = FakeBridgeService();
    final list = await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    final day = DateTime(2026, 7, 13);
    final completedAt = DateTime(2026, 7, 13, 10).millisecondsSinceEpoch;
    final task = await fake.createTask(listId: list.id, title: 'Outcome');
    fake.setTaskCalendarStateForTest(
      taskId: task.id,
      status: 'done',
      completedAt: completedAt,
    );
    final container = ProviderContainer(
      overrides: [bridgeServiceProvider.overrideWithValue(fake)],
    );
    addTearDown(container.dispose);
    final range = CalendarRange.day(day);
    final occurrence = (await container.read(
      calendarOccurrencesProvider(range).future,
    )).single;

    expect(
      () => container
          .read(calendarOccurrencesProvider(range).notifier)
          .moveOccurrence(
            occurrence: occurrence,
            targetDate: DateTime(2026, 7, 14),
          ),
      throwsStateError,
    );
    expect(fake.updateTaskCalls, isEmpty);
  });

  test('datetime due move rejects a DST gap in its saved timezone', () async {
    final fake = FakeBridgeService();
    final list = await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    final originalDue = dateTimeDue(
      localDateTime: DateTime(2025, 3, 8, 2, 30),
      timeZone: 'America/New_York',
    );
    final task = await fake.createTask(
      listId: list.id,
      title: 'DST gap',
      due: originalDue,
    );
    final occurrence = CalendarOccurrenceDto(
      task: task,
      listName: list.name,
      listArchived: false,
      kind: switch (originalDue) {
        TaskDueDto_DateTime(:final dueAt, :final timeZone) =>
          CalendarOccurrenceKindDto.dateTimeDue(
            dueAt: dueAt,
            timeZone: timeZone,
          ),
        _ => throw StateError('expected datetime due'),
      },
    );
    final container = ProviderContainer(
      overrides: [bridgeServiceProvider.overrideWithValue(fake)],
    );
    addTearDown(container.dispose);
    final range = CalendarRange.day(DateTime(2025, 3, 8));

    expect(
      () => container
          .read(calendarOccurrencesProvider(range).notifier)
          .moveOccurrence(
            occurrence: occurrence,
            targetDate: DateTime(2025, 3, 9),
          ),
      throwsFormatException,
    );
    expect(fake.updateTaskCalls, isEmpty);
  });

  test('task mutations and sync invalidate cached calendar ranges', () async {
    final fake = _CountingCalendarFake();
    final list = await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
    final range = CalendarRange.day(DateTime.now());
    final container = ProviderContainer(
      overrides: [bridgeServiceProvider.overrideWithValue(fake)],
    );
    addTearDown(container.dispose);

    await container.read(calendarOccurrencesProvider(range).future);
    expect(fake.calendarReads, 1);
    await container
        .read(tasksProvider(list.id).notifier)
        .createTask('Created', due: dateOnlyDue(DateTime.now()));
    await container.read(calendarOccurrencesProvider(range).future);
    expect(fake.calendarReads, 2);
    final created = (await fake.getTasks(listId: list.id)).single;
    await container
        .read(tasksProvider(list.id).notifier)
        .updateTask(
          taskId: created.id,
          title: 'Changed',
          note: created.note,
          priority: created.priority,
          due: created.due,
          scheduledAt: created.scheduledAt,
          estimatedMinutes: created.estimatedMinutes,
        );
    await container.read(calendarOccurrencesProvider(range).future);
    expect(fake.calendarReads, 3);
    await container
        .read(tasksProvider(list.id).notifier)
        .setStatus(created.id, 'done');
    await container.read(calendarOccurrencesProvider(range).future);
    expect(fake.calendarReads, 4);
    await container.read(syncStatusProvider.notifier).syncNow();
    await container.read(calendarOccurrencesProvider(range).future);
    expect(fake.calendarReads, 5);
  });
}

void _expectCivilBoundaries(
  tz.Location location,
  ({int todayStartMs, int tomorrowStartMs, int dayAfterTomorrowStartMs}) range,
  List<(int, int, int)> expected,
) {
  final boundaries = [
    range.todayStartMs,
    range.tomorrowStartMs,
    range.dayAfterTomorrowStartMs,
  ].map((value) => tz.TZDateTime.fromMillisecondsSinceEpoch(location, value));
  expect(
    boundaries
        .map((value) => (value.year, value.month, value.day))
        .toList(growable: false),
    expected,
  );
  expect(boundaries.every((value) => value.hour == 0), isTrue);
}

class _CountingCalendarFake extends FakeBridgeService {
  int calendarReads = 0;

  @override
  Future<List<CalendarOccurrenceDto>> getCalendarOccurrences({
    required CalendarRangeInput range,
  }) {
    calendarReads += 1;
    return super.getCalendarOccurrences(range: range);
  }
}

import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:intl/intl.dart' hide TextDirection;
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:taskveil/main.dart';
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/screens/calendar_screen.dart';
import 'package:taskveil/src/ui/task_components.dart';

import 'support/fake_bridge_service.dart';

void main() {
  testWidgets(
    'Calendar navigation shows dual occurrences and completes both together',
    (tester) async {
      final fake = FakeBridgeService();
      final listId = await _createInbox(fake);
      final now = DateTime.now();
      final today = DateTime(now.year, now.month, now.day);
      final scheduledAt = DateTime(
        today.year,
        today.month,
        today.day,
        10,
      ).millisecondsSinceEpoch;
      final dual = await fake.createTask(
        listId: listId,
        title: 'Dual calendar task',
        due: testDateOnlyDueFromMillis(today.millisecondsSinceEpoch),
      );
      fake.setScheduledAtForTest(taskId: dual.id, scheduledAt: scheduledAt);
      final directOccurrences = await fake.getCalendarOccurrences(
        range: CalendarRange.day(today).toInput(),
      );
      expect(directOccurrences, hasLength(2));
      expect(
        directOccurrences.map(CalendarOccurrenceKey.fromOccurrence).toSet(),
        hasLength(2),
      );

      await _pumpApp(tester, fake);
      await _openCalendar(tester);

      final dueKey = CalendarOccurrenceKey.fromOccurrence(
        directOccurrences.singleWhere(
          (occurrence) => occurrence.kind is CalendarOccurrenceKindDto_DateDue,
        ),
      );
      final scheduledKey = CalendarOccurrenceKey.fromOccurrence(
        directOccurrences.singleWhere(
          (occurrence) =>
              occurrence.kind is CalendarOccurrenceKindDto_Scheduled,
        ),
      );
      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$dueKey')),
        findsOneWidget,
      );
      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$scheduledKey')),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('calendar-mode-week')), findsOneWidget);

      await tester.tap(
        find.byKey(ValueKey('calendar-occurrence-check-$dueKey')),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$dueKey')),
        findsOneWidget,
      );
      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$scheduledKey')),
        findsOneWidget,
      );
      expect(
        tester
            .widget<AppHomeTaskRow>(
              find.byKey(ValueKey('calendar-occurrence-row-$dueKey')),
            )
            .isDone,
        isTrue,
      );
      expect(
        tester
            .widget<AppHomeTaskRow>(
              find.byKey(ValueKey('calendar-occurrence-row-$scheduledKey')),
            )
            .isDone,
        isTrue,
      );
      expect(
        find.byKey(const ValueKey('task-completion-halo')),
        findsNWidgets(2),
      );
      expect(
        find.byKey(const ValueKey('task-strikethrough-overlay')),
        findsNWidgets(2),
      );
      expect(
        find.byKey(const ValueKey('calendar-completed-toggle')),
        findsNothing,
      );

      await tester.pump(const Duration(milliseconds: 500));
      expect(
        find.byKey(ValueKey('calendar-completion-exit-$dueKey')),
        findsOneWidget,
      );
      expect(
        find.byKey(ValueKey('calendar-completion-exit-$scheduledKey')),
        findsOneWidget,
      );
      await tester.pump(const Duration(milliseconds: 500));

      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$dueKey')),
        findsNothing,
      );
      expect(
        find.byKey(ValueKey('calendar-occurrence-row-$scheduledKey')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('calendar-completed-toggle')),
        findsOneWidget,
      );
      expect((await fake.getTasks(listId: listId)).single.status, 'done');

      await tester.tap(find.byIcon(LucideIcons.house300));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('completed-section-toggle')),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('completed-section-toggle')));
      await tester.pumpAndSettle();
      expect(find.text('Dual calendar task'), findsOneWidget);
    },
  );

  testWidgets('Calendar rows preserve a three-level task tree', (tester) async {
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    final today = _today();
    final due = testDateOnlyDueFromMillis(today.millisecondsSinceEpoch);
    final parent = await fake.createTask(
      listId: listId,
      title: 'Calendar parent',
      due: due,
    );
    final child = await fake.createTask(
      listId: listId,
      title: 'Calendar child',
      parentTaskId: parent.id,
      due: due,
    );
    final grandchild = await fake.createTask(
      listId: listId,
      title: 'Calendar grandchild',
      parentTaskId: child.id,
      due: due,
    );

    await _pumpApp(tester, fake);
    await _openCalendar(tester);

    CalendarOccurrenceKey keyFor(String taskId) => CalendarOccurrenceKey(
      taskId: taskId,
      kind: 'date_due',
      marker: _civilDate(today),
    );

    final parentRow = tester.widget<AppHomeTaskRow>(
      find.byKey(ValueKey('calendar-occurrence-row-${keyFor(parent.id)}')),
    );
    final childRow = tester.widget<AppHomeTaskRow>(
      find.byKey(ValueKey('calendar-occurrence-row-${keyFor(child.id)}')),
    );
    final grandchildRow = tester.widget<AppHomeTaskRow>(
      find.byKey(ValueKey('calendar-occurrence-row-${keyFor(grandchild.id)}')),
    );
    expect(parentRow.depth, 0);
    expect(childRow.depth, 1);
    expect(childRow.parentTaskName, 'Calendar parent');
    expect(grandchildRow.depth, 2);
    expect(grandchildRow.parentTaskName, 'Calendar child');
    expect(
      find.byKey(
        ValueKey('calendar-hierarchy-horizontal-${keyFor(grandchild.id)}'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('Month selection and detail return preserve Calendar state', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    final today = _today();
    final tomorrow = DateTime(today.year, today.month, today.day + 1);
    await fake.createTask(
      listId: listId,
      title: 'Tomorrow calendar detail',
      due: testDateOnlyDueFromMillis(tomorrow.millisecondsSinceEpoch),
    );

    await _pumpApp(tester, fake);
    await _openCalendar(tester);
    await tester.tap(find.byKey(const ValueKey('calendar-mode-month')));
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(ValueKey('calendar-month-day-${_civilDate(tomorrow)}')),
    );
    await tester.pumpAndSettle();

    expect(find.text('Tomorrow calendar detail'), findsOneWidget);
    await tester.ensureVisible(find.text('Tomorrow calendar detail'));
    await tester.tap(find.text('Tomorrow calendar detail'));
    await tester.pumpAndSettle();
    expect(find.byType(CalendarScreen), findsNothing);
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('calendar-month-grid')), findsOneWidget);
    expect(find.text('Tomorrow calendar detail'), findsOneWidget);
  });

  testWidgets('dragging due occurrence changes only due, not scheduled', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    final today = _today();
    final scheduledAt = DateTime(
      today.year,
      today.month,
      today.day,
      11,
    ).millisecondsSinceEpoch;
    final task = await fake.createTask(
      listId: listId,
      title: 'Drag only due',
      due: testDateOnlyDueFromMillis(today.millisecondsSinceEpoch),
    );
    fake.setScheduledAtForTest(taskId: task.id, scheduledAt: scheduledAt);
    final dueKey = CalendarOccurrenceKey(
      taskId: task.id,
      kind: 'date_due',
      marker: _civilDate(today),
    );

    await _pumpApp(tester, fake);
    await _openCalendar(tester);
    final targetKey = tester
        .widgetList<Widget>(
          find.byWidgetPredicate((widget) {
            final key = widget.key;
            return key is ValueKey<String> &&
                key.value.startsWith('calendar-week-day-') &&
                key.value != 'calendar-week-day-${_civilDate(today)}';
          }),
        )
        .map((widget) => (widget.key! as ValueKey<String>).value)
        .first;
    final targetDate = DateTime.parse(
      targetKey.substring('calendar-week-day-'.length),
    );
    final draggable = find.byKey(ValueKey('calendar-draggable-$dueKey'));
    final target = find.byKey(ValueKey(targetKey));
    final gesture = await tester.startGesture(tester.getCenter(draggable));
    await tester.pump(kLongPressTimeout + const Duration(milliseconds: 100));
    await gesture.moveTo(tester.getCenter(target));
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();

    final updated = (await fake.getTasks(listId: listId)).single;
    expect(
      updated.due,
      testDateOnlyDueFromMillis(targetDate.millisecondsSinceEpoch),
    );
    expect(updated.scheduledAt, scheduledAt);
  });

  testWidgets('Calendar applies Monday and Sunday week starts', (tester) async {
    final fake = FakeBridgeService();
    await _createInbox(fake);
    await fake.setFrontendSetting(
      key: calendarWeekStartSettingKey,
      value: mondayCalendarWeekStart,
    );
    final today = _today();
    final monday = DateTime(
      today.year,
      today.month,
      today.day - (today.weekday - DateTime.monday),
    );
    final sundayAfterMonday = DateTime(
      monday.year,
      monday.month,
      monday.day + 6,
    );

    await _pumpCalendarScreen(tester, fake, settle: true);

    expect(
      tester
          .getTopLeft(
            find.byKey(ValueKey('calendar-week-day-${_civilDate(monday)}')),
          )
          .dx,
      lessThan(
        tester
            .getTopLeft(
              find.byKey(
                ValueKey('calendar-week-day-${_civilDate(sundayAfterMonday)}'),
              ),
            )
            .dx,
      ),
    );

    final container = ProviderScope.containerOf(
      tester.element(find.byType(CalendarScreen)),
    );
    await container
        .read(calendarWeekStartProvider.notifier)
        .setWeekStart(sundayCalendarWeekStart);
    await tester.pumpAndSettle();
    final sunday = DateTime(
      today.year,
      today.month,
      today.day - (today.weekday % 7),
    );
    final saturdayAfterSunday = DateTime(
      sunday.year,
      sunday.month,
      sunday.day + 6,
    );

    expect(
      tester
          .getTopLeft(
            find.byKey(ValueKey('calendar-week-day-${_civilDate(sunday)}')),
          )
          .dx,
      lessThan(
        tester
            .getTopLeft(
              find.byKey(
                ValueKey(
                  'calendar-week-day-${_civilDate(saturdayAfterSunday)}',
                ),
              ),
            )
            .dx,
      ),
    );
  });

  testWidgets(
    'move menu semantics names kind and task and preserves sibling occurrence',
    (tester) async {
      final fake = FakeBridgeService();
      final listId = await _createInbox(fake);
      final today = _today();
      final scheduledAt = DateTime(
        today.year,
        today.month,
        today.day,
        9,
      ).millisecondsSinceEpoch;
      final task = await fake.createTask(
        listId: listId,
        title: 'Accessible move',
        due: testDateOnlyDueFromMillis(today.millisecondsSinceEpoch),
      );
      fake.setScheduledAtForTest(taskId: task.id, scheduledAt: scheduledAt);

      await _pumpApp(tester, fake);
      await _openCalendar(tester);
      final semantics = tester.ensureSemantics();
      expect(
        find.semantics.byLabel('Due: change date for Accessible move'),
        findsOneWidget,
      );
      expect(
        find.semantics.byLabel('Planned: change date for Accessible move'),
        findsOneWidget,
      );
      semantics.dispose();

      final dueKey = CalendarOccurrenceKey(
        taskId: task.id,
        kind: 'date_due',
        marker: _civilDate(today),
      );
      await tester.tap(find.byKey(ValueKey('calendar-move-menu-$dueKey')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Tomorrow').last);
      await tester.pumpAndSettle();

      final tomorrow = DateTime(today.year, today.month, today.day + 1);
      final updated = (await fake.getTasks(listId: listId)).single;
      expect(
        updated.due,
        testDateOnlyDueFromMillis(tomorrow.millisecondsSinceEpoch),
      );
      expect(updated.scheduledAt, scheduledAt);
    },
  );

  testWidgets('datetime due shows saved timezone wall time and offset', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    final today = _today();
    final dueAt = DateTime.utc(today.year, today.month, today.day, 12);
    final due = testDateTimeDueFromMillis(
      dueAt.millisecondsSinceEpoch,
      timeZone: 'America/New_York',
    );
    await fake.createTask(
      listId: listId,
      title: 'Saved timezone deadline',
      due: due,
    );
    final savedZoneDate = taskDueDisplayDate(due);
    final expectedTime = DateFormat.jm('en').format(savedZoneDate);
    final expectedZone =
        'America/New_York (${taskDueUtcOffsetLabel(savedZoneDate)})';

    await _pumpCalendarScreen(tester, fake, settle: true);

    expect(find.textContaining(expectedTime), findsOneWidget);
    expect(find.textContaining(expectedZone), findsOneWidget);
    final semantics = tester.ensureSemantics();
    expect(
      find.semantics.byPredicate(
        (node) =>
            node.getSemanticsData().label.contains(expectedTime) &&
            node.getSemanticsData().label.contains(expectedZone),
      ),
      findsWidgets,
    );
    semantics.dispose();
  });

  testWidgets('Calendar uses two panes at 1024 and one pane at 720', (
    tester,
  ) async {
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    await fake.createTask(
      listId: listId,
      title: 'Responsive calendar task',
      due: testDateOnlyDueFromMillis(_today().millisecondsSinceEpoch),
    );
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1024, 760);
    await _pumpCalendarScreen(tester, fake, settle: true);
    await tester.tap(find.byKey(const ValueKey('calendar-mode-month')));
    await tester.pumpAndSettle();
    expect(find.byType(VerticalDivider), findsOneWidget);
    expect(find.byKey(const ValueKey('calendar-month-grid')), findsOneWidget);
    expect(find.text('Responsive calendar task'), findsOneWidget);

    tester.view.physicalSize = const Size(720, 760);
    await tester.pumpAndSettle();
    expect(find.byType(VerticalDivider), findsNothing);
    expect(find.text('Responsive calendar task'), findsOneWidget);
  });

  testWidgets('Calendar shows a quiet empty agenda', (tester) async {
    final fake = FakeBridgeService();
    await _createInbox(fake);
    await _pumpCalendarScreen(tester, fake, settle: true);

    expect(find.text('Nothing planned.'), findsOneWidget);
    expect(
      find.text(
        'Choose another day or capture what you want to make time for.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('Calendar completion is immediate with Reduce Motion', (
    tester,
  ) async {
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    final today = _today();
    final task = await fake.createTask(
      listId: listId,
      title: 'Immediate calendar completion',
      due: testDateOnlyDueFromMillis(today.millisecondsSinceEpoch),
    );
    final key = CalendarOccurrenceKey(
      taskId: task.id,
      kind: 'date_due',
      marker: _civilDate(today),
    );

    await _pumpCalendarScreen(tester, fake, settle: true);
    await tester.tap(find.byKey(ValueKey('calendar-occurrence-check-$key')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.byKey(ValueKey('calendar-completion-exit-$key')), findsNothing);
    expect(find.byKey(const ValueKey('task-completion-halo')), findsNothing);
    expect(
      find.byKey(const ValueKey('calendar-completed-toggle')),
      findsOneWidget,
    );
  });

  testWidgets('Calendar remains usable at 320px Japanese text scale 2 RTL', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(320, 720);
    tester.view.devicePixelRatio = 1;
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
      tester.platformDispatcher.clearTextScaleFactorTestValue();
    });
    final fake = FakeBridgeService();
    final listId = await _createInbox(fake);
    await fake.createTask(
      listId: listId,
      title: '狭い画面でも読みやすい予定タスク',
      due: testDateOnlyDueFromMillis(_today().millisecondsSinceEpoch),
    );

    await tester.pumpWidget(
      ProviderScope(
        overrides: [bridgeServiceProvider.overrideWithValue(fake)],
        child: MaterialApp(
          locale: const Locale('ja'),
          supportedLocales: AppLocalizations.supportedLocales,
          localizationsDelegates: const [
            AppLocalizations.delegate,
            GlobalMaterialLocalizations.delegate,
            GlobalWidgetsLocalizations.delegate,
            GlobalCupertinoLocalizations.delegate,
          ],
          home: const Directionality(
            textDirection: TextDirection.rtl,
            child: CalendarScreen(),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('カレンダー'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('calendar-mode-week')),
        matching: find.text('週'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('calendar-mode-month')),
        matching: find.text('月'),
      ),
      findsOneWidget,
    );
    expect(find.text('今日'), findsOneWidget);
    expect(find.text('狭い画面でも読みやすい予定タスク'), findsOneWidget);
    final titleRect = tester.getRect(find.text('カレンダー'));
    expect(titleRect.left, greaterThanOrEqualTo(0));
    expect(titleRect.right, lessThanOrEqualTo(320));
    final modeRect = tester.getRect(
      find.byKey(const ValueKey('calendar-header-mode-row')),
    );
    final periodRect = tester.getRect(
      find.byKey(const ValueKey('calendar-header-period-row')),
    );
    expect(periodRect.top, greaterThanOrEqualTo(modeRect.bottom));
    expect(tester.takeException(), isNull);
  });

  testWidgets('Calendar exposes loading and retryable error states', (
    tester,
  ) async {
    final pending = _PendingCalendarBridge();
    await _createInbox(pending);
    await _pumpCalendarScreen(tester, pending);
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.byType(CircularProgressIndicator), findsOneWidget);

    pending.completeError();
    await tester.pumpAndSettle();
    expect(find.text('Calendar could not be loaded.'), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
  });
}

class _PendingCalendarBridge extends FakeBridgeService {
  final Completer<List<CalendarOccurrenceDto>> _calendar = Completer();

  @override
  Future<List<CalendarOccurrenceDto>> getCalendarOccurrences({
    required CalendarRangeInput range,
  }) => _calendar.future;

  void completeError() => _calendar.completeError(StateError('calendar test'));
}

Future<String> _createInbox(FakeBridgeService fake) async {
  await fake.createDefaultList(name: 'Inbox', sortOrder: 'a0');
  return (await fake.getLists()).single.id;
}

Future<void> _pumpApp(WidgetTester tester, BridgeService bridge) async {
  await tester.pumpWidget(
    TaskveilApp(overrides: [bridgeServiceProvider.overrideWithValue(bridge)]),
  );
  await tester.pumpAndSettle();
}

Future<void> _pumpCalendarScreen(
  WidgetTester tester,
  BridgeService bridge, {
  bool settle = false,
}) async {
  await tester.pumpWidget(
    ProviderScope(
      overrides: [bridgeServiceProvider.overrideWithValue(bridge)],
      child: MaterialApp(
        supportedLocales: AppLocalizations.supportedLocales,
        localizationsDelegates: const [
          AppLocalizations.delegate,
          GlobalMaterialLocalizations.delegate,
          GlobalWidgetsLocalizations.delegate,
          GlobalCupertinoLocalizations.delegate,
        ],
        home: const CalendarScreen(),
      ),
    ),
  );
  if (settle) {
    await tester.pumpAndSettle();
  } else {
    await tester.pump();
  }
}

Future<void> _openCalendar(WidgetTester tester) async {
  await tester.tap(find.byIcon(LucideIcons.calendarDays300));
  await tester.pumpAndSettle();
}

DateTime _today() {
  final now = DateTime.now();
  return DateTime(now.year, now.month, now.day);
}

String _civilDate(DateTime value) =>
    '${value.year.toString().padLeft(4, '0')}-'
    '${value.month.toString().padLeft(2, '0')}-'
    '${value.day.toString().padLeft(2, '0')}';

import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';

const _todayStartMs = 1788220800000;
const _tomorrowStartMs = _todayStartMs + 86400000;
const _homeMedianBudget = Duration(milliseconds: 1500);
const _calendarMedianBudget = Duration(milliseconds: 750);

void main() {
  late Directory profile;

  setUpAll(() async {
    profile = await Directory.systemTemp.createTemp(
      'taskveil_home_calendar_performance_',
    );
    final seedExecutable = File(
      '../target/debug/examples/seed_home_calendar_performance_fixture',
    );
    final result = await Process.run(
      seedExecutable.path,
      [profile.path],
      environment: {...Platform.environment, 'FLUTTER_TEST': '1'},
    );
    expect(
      result.exitCode,
      0,
      reason: 'fixture seed failed:\n${result.stdout}\n${result.stderr}',
    );

    await RustLib.init(
      externalLibrary: ExternalLibrary.open(
        'rust/target/release/libtaskveil_app_bridge.dylib',
      ),
    );
    await initCore(dbDir: profile.path, defaultInboxName: 'Inbox');
  });

  tearDownAll(() async {
    RustLib.dispose();
    await profile.delete(recursive: true);
  });

  test(
    'real FRB Home and Calendar stay within 10000-task SQLCipher budgets',
    () async {
      CalendarRangeInput range() => CalendarRangeInput(
        startOn: '2026-09-01',
        endOn: '2026-09-08',
        startAt: DateTime.fromMillisecondsSinceEpoch(
          _todayStartMs,
          isUtc: true,
        ),
        endAt: DateTime.fromMillisecondsSinceEpoch(
          _todayStartMs + 7 * 86400000,
          isUtc: true,
        ),
      );

      await getHomeTasks(
        todayStartMs: _todayStartMs,
        tomorrowStartMs: _tomorrowStartMs,
      );
      await getCalendarOccurrences(range: range());

      final homeSamples = <Duration>[];
      final calendarSamples = <Duration>[];
      var homeRows = 0;
      var calendarRows = 0;
      for (var sample = 0; sample < 5; sample++) {
        final homeWatch = Stopwatch()..start();
        homeRows = (await getHomeTasks(
          todayStartMs: _todayStartMs,
          tomorrowStartMs: _tomorrowStartMs,
        )).length;
        homeWatch.stop();
        homeSamples.add(homeWatch.elapsed);

        final calendarWatch = Stopwatch()..start();
        calendarRows = (await getCalendarOccurrences(range: range())).length;
        calendarWatch.stop();
        calendarSamples.add(calendarWatch.elapsed);
      }
      homeSamples.sort();
      calendarSamples.sort();
      final homeMedian = homeSamples[homeSamples.length ~/ 2];
      final calendarMedian = calendarSamples[calendarSamples.length ~/ 2];

      // Visible in CI logs when run with `flutter test --reporter expanded`.
      // ignore: avoid_print
      print(
        'FRB SQLCipher 10k benchmark: home_rows=$homeRows '
        'home_median_ms=${homeMedian.inMilliseconds} '
        'calendar_rows=$calendarRows '
        'calendar_median_ms=${calendarMedian.inMilliseconds}',
      );
      expect(homeRows, greaterThan(1000));
      expect(calendarRows, greaterThan(1000));
      expect(
        homeMedian,
        lessThanOrEqualTo(_homeMedianBudget),
        reason: 'Home FRB median exceeded $_homeMedianBudget',
      );
      expect(
        calendarMedian,
        lessThanOrEqualTo(_calendarMedianBudget),
        reason: 'Calendar FRB median exceeded $_calendarMedianBudget',
      );
    },
    timeout: const Timeout(Duration(minutes: 2)),
  );
}

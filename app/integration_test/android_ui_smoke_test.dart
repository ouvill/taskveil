import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider/path_provider.dart';
import 'package:taskveil/main.dart';
import 'package:taskveil/src/core/providers.dart';
import 'package:taskveil/src/core/task_due.dart';
import 'package:taskveil/src/router.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';
import 'package:taskveil/src/screens/calendar_screen.dart';
import 'package:taskveil/src/screens/home_screen.dart';
import 'package:taskveil/src/screens/lists_screen.dart';
import 'package:taskveil/src/screens/menu_screen.dart';
import 'package:taskveil/src/screens/templates_screen.dart';

const _runId = String.fromEnvironment('TASKVEIL_ANDROID_PARITY_RUN_ID');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('renders the primary Flutter surfaces with the Android core', (
    tester,
  ) async {
    expect(_runId, isNotEmpty);
    await RustLib.init();
    final support = await getApplicationSupportDirectory();
    final profile = Directory('${support.path}/android-ui-smoke-$_runId');
    await profile.create(recursive: true);
    await initCore(dbDir: profile.path, defaultInboxName: 'Inbox');
    await setFrontendSetting(key: onboardingCompletedSettingKey, value: '1');

    final inbox = (await getLists()).singleWhere((list) => list.isDefault);
    final taskTitle = 'Android UI smoke $_runId';
    await createTask(
      listId: inbox.id,
      title: taskTitle,
      due: taskDueInput(dateOnlyDue(DateTime.now())),
    );

    final router = buildAppRouter();
    addTearDown(router.dispose);
    await tester.pumpWidget(TaskveilApp(router: router));
    await tester.pumpAndSettle();

    expect(find.byType(HomeScreen), findsOneWidget);
    expect(find.text(taskTitle), findsOneWidget);

    router.go('/calendar');
    await tester.pumpAndSettle();
    expect(find.byType(CalendarScreen), findsOneWidget);

    router.go('/lists');
    await tester.pumpAndSettle();
    expect(find.byType(ListsScreen), findsOneWidget);
    expect(find.text('Inbox'), findsWidgets);

    router.go('/templates');
    await tester.pumpAndSettle();
    expect(find.byType(TemplatesScreen), findsOneWidget);

    router.go('/menu');
    await tester.pumpAndSettle();
    expect(find.byType(MenuScreen), findsOneWidget);
  });
}

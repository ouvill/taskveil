import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';

void main() {
  late Directory profileDirectory;

  setUpAll(() async {
    await RustLib.init(
      externalLibrary: ExternalLibrary.open(
        'rust/target/release/libtaskveil_app_bridge.dylib',
      ),
    );
    profileDirectory = await Directory.systemTemp.createTemp(
      'taskveil-frb-profile-',
    );
    await initCore(dbDir: profileDirectory.path, defaultInboxName: 'Inbox');
  });

  tearDownAll(() async {
    RustLib.dispose();
    await profileDirectory.delete(recursive: true);
  });

  test('encrypted profile supports real list and task CRUD', () async {
    final list = await createList(name: 'Bridge list', sortOrder: 'ignored');
    final task = await createTask(listId: list.id, title: 'Bridge task');

    final tasks = await getTasks(listId: list.id);
    expect(tasks.map((value) => value.id), contains(task.id));
    expect(tasks.single.title, 'Bridge task');

    await deleteTask(taskId: task.id);
    expect(await getTasks(listId: list.id), isEmpty);
  });

  test('typed bridge error redacts invalid input', () async {
    await expectLater(
      deleteTask(taskId: 'not-a-uuid secret/input'),
      throwsA(
        isA<BridgeErrorDto>()
            .having(
              (error) => error.code,
              'code',
              BridgeErrorCodeDto.invalidInput,
            )
            .having((error) => error.arguments, 'arguments', isEmpty)
            .having((error) => error.retryable, 'retryable', isFalse),
      ),
    );
  });
}

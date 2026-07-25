import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider/path_provider.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/rust/frb_generated.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('rotates DK and immediately reopens SQLCipher with the new key', (
    tester,
  ) async {
    await RustLib.init();
    final support = await getApplicationSupportDirectory();
    final profile = Directory('${support.path}/dk-rotation-platform-test-v4');
    final restartSentinel = File('${profile.path}/first-rotation-complete');
    final reopeningAfterRestart = await restartSentinel.exists();
    await profile.create(recursive: true);
    await initCore(dbDir: profile.path, defaultInboxName: 'Inbox');

    final generation = await rotateDeviceKey();
    expect(generation, greaterThan(reopeningAfterRestart ? 2 : 1));
    expect(await getLists(), isNotEmpty);
    await restartSentinel.writeAsString('$generation\n', flush: true);
  });
}

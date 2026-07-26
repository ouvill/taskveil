import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/core/safe_startup_log.dart';
import 'package:taskveil/src/rust/api.dart';

void main() {
  test('startup logs contain only fixed event and typed code', () {
    const secret = '/private/profile/alice/taskveil.db?token=secret';
    const error = BridgeErrorDto(
      code: BridgeErrorCodeDto.storageFailure,
      arguments: [],
      retryable: false,
    );

    final message = startupFailureLogMessage(
      StartupFailureEvent.nativeCore,
      error,
    );

    expect(
      message,
      'Taskveil startup failure event=native_core code=storage_failure',
    );
    expect(message, isNot(contains(secret)));
  });

  test(
    'unknown startup failures use fixed internal code without raw payload',
    () {
      const secret = '/private/profile/alice/taskveil.db?token=secret';

      final message = startupFailureLogMessage(
        StartupFailureEvent.reminderNotifications,
        StateError(secret),
      );

      expect(
        message,
        'Taskveil startup failure '
        'event=reminder_notifications code=internal',
      );
      expect(message, isNot(contains(secret)));
      expect(message, isNot(contains('StateError')));
    },
  );
}

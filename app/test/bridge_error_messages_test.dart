import 'dart:ui' show Locale;

import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';

void main() {
  final l10n = lookupAppLocalizations(const Locale('en'));

  test('bridge errors localize from stable code only', () {
    const error = BridgeErrorDto(
      code: BridgeErrorCodeDto.storageFailure,
      arguments: [],
      retryable: false,
    );

    expect(bridgeErrorMessage(l10n, error), l10n.bridgeErrorStorageFailure);
  });

  test('unknown exceptions never expose their raw text', () {
    const secret = '/private/profile/alice/taskveil.db?token=secret';
    final message = bridgeErrorMessage(l10n, StateError(secret));

    expect(message, l10n.bridgeErrorInternal);
    expect(message, isNot(contains(secret)));
  });
}

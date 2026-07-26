import 'dart:ui' show Locale;

import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';
import 'package:taskveil/src/ui/bridge_error_messages.dart';

void main() {
  final l10n = lookupAppLocalizations(const Locale('en'));

  test('every bridge error localizes from its stable code only', () {
    final expected = <BridgeErrorCodeDto, String>{
      BridgeErrorCodeDto.invalidInput: l10n.bridgeErrorInvalidInput,
      BridgeErrorCodeDto.notFound: l10n.bridgeErrorNotFound,
      BridgeErrorCodeDto.conflict: l10n.bridgeErrorConflict,
      BridgeErrorCodeDto.unauthorized: l10n.bridgeErrorUnauthorized,
      BridgeErrorCodeDto.credentialUnavailable:
          l10n.bridgeErrorCredentialUnavailable,
      BridgeErrorCodeDto.accountBoundUnavailable:
          l10n.bridgeErrorAccountBoundUnavailable,
      BridgeErrorCodeDto.entitlementRequired:
          l10n.bridgeErrorEntitlementRequired,
      BridgeErrorCodeDto.upgradeRequired: l10n.bridgeErrorUpgradeRequired,
      BridgeErrorCodeDto.busy: l10n.bridgeErrorBusy,
      BridgeErrorCodeDto.leaseLost: l10n.bridgeErrorLeaseLost,
      BridgeErrorCodeDto.clockSkew: l10n.bridgeErrorClockSkew,
      BridgeErrorCodeDto.cryptoUnavailable: l10n.bridgeErrorCryptoUnavailable,
      BridgeErrorCodeDto.storageFailure: l10n.bridgeErrorStorageFailure,
      BridgeErrorCodeDto.syncFailure: l10n.bridgeErrorSyncFailure,
      BridgeErrorCodeDto.internal: l10n.bridgeErrorInternal,
    };

    expect(expected.length, BridgeErrorCodeDto.values.length);
    for (final code in BridgeErrorCodeDto.values) {
      final error = BridgeErrorDto(
        code: code,
        arguments: const [],
        retryable: false,
      );
      expect(bridgeErrorMessage(l10n, error), expected[code], reason: '$code');
    }
  });

  test('unknown exceptions never expose their raw text', () {
    const secret = '/private/profile/alice/taskveil.db?token=secret';
    final message = bridgeErrorMessage(l10n, StateError(secret));

    expect(message, l10n.bridgeErrorInternal);
    expect(message, isNot(contains(secret)));
  });
}

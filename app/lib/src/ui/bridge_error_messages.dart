import 'package:taskveil/src/generated/l10n/app_localizations.dart';
import 'package:taskveil/src/rust/api.dart';

/// Converts only the closed bridge error contract into user-facing copy.
///
/// Unknown Dart/plugin failures deliberately use a generic message; calling
/// `toString()` here would reintroduce paths, database details, server bodies,
/// or user input into UI text.
String bridgeErrorMessage(AppLocalizations l10n, Object error) {
  if (error is! BridgeErrorDto) {
    return l10n.bridgeErrorInternal;
  }
  return switch (error.code) {
    BridgeErrorCodeDto.invalidInput => l10n.bridgeErrorInvalidInput,
    BridgeErrorCodeDto.notFound => l10n.bridgeErrorNotFound,
    BridgeErrorCodeDto.conflict => l10n.bridgeErrorConflict,
    BridgeErrorCodeDto.unauthorized => l10n.bridgeErrorUnauthorized,
    BridgeErrorCodeDto.credentialUnavailable =>
      l10n.bridgeErrorCredentialUnavailable,
    BridgeErrorCodeDto.accountBoundUnavailable =>
      l10n.bridgeErrorAccountBoundUnavailable,
    BridgeErrorCodeDto.entitlementRequired =>
      l10n.bridgeErrorEntitlementRequired,
    BridgeErrorCodeDto.upgradeRequired => l10n.bridgeErrorUpgradeRequired,
    BridgeErrorCodeDto.busy => l10n.bridgeErrorBusy,
    BridgeErrorCodeDto.leaseLost => l10n.bridgeErrorLeaseLost,
    BridgeErrorCodeDto.clockSkew => l10n.bridgeErrorClockSkew,
    BridgeErrorCodeDto.cryptoUnavailable => l10n.bridgeErrorCryptoUnavailable,
    BridgeErrorCodeDto.storageFailure => l10n.bridgeErrorStorageFailure,
    BridgeErrorCodeDto.syncFailure => l10n.bridgeErrorSyncFailure,
    BridgeErrorCodeDto.internal => l10n.bridgeErrorInternal,
  };
}

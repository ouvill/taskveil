import 'package:flutter/foundation.dart';
import 'package:taskveil/src/rust/api.dart';

enum StartupFailureEvent {
  reminderNotifications,
  timerNotifications,
  nativeCore,
}

void logStartupFailure(StartupFailureEvent event, Object error) {
  debugPrint(startupFailureLogMessage(event, error));
}

@visibleForTesting
String startupFailureLogMessage(StartupFailureEvent event, Object error) {
  final eventName = switch (event) {
    StartupFailureEvent.reminderNotifications => 'reminder_notifications',
    StartupFailureEvent.timerNotifications => 'timer_notifications',
    StartupFailureEvent.nativeCore => 'native_core',
  };
  final code = error is BridgeErrorDto
      ? switch (error.code) {
          BridgeErrorCodeDto.invalidInput => 'invalid_input',
          BridgeErrorCodeDto.notFound => 'not_found',
          BridgeErrorCodeDto.conflict => 'conflict',
          BridgeErrorCodeDto.unauthorized => 'unauthorized',
          BridgeErrorCodeDto.credentialUnavailable => 'credential_unavailable',
          BridgeErrorCodeDto.accountBoundUnavailable =>
            'account_bound_unavailable',
          BridgeErrorCodeDto.entitlementRequired => 'entitlement_required',
          BridgeErrorCodeDto.upgradeRequired => 'upgrade_required',
          BridgeErrorCodeDto.busy => 'busy',
          BridgeErrorCodeDto.leaseLost => 'lease_lost',
          BridgeErrorCodeDto.clockSkew => 'clock_skew',
          BridgeErrorCodeDto.cryptoUnavailable => 'crypto_unavailable',
          BridgeErrorCodeDto.storageFailure => 'storage_failure',
          BridgeErrorCodeDto.syncFailure => 'sync_failure',
          BridgeErrorCodeDto.internal => 'internal',
        }
      : 'internal';
  return 'Taskveil startup failure event=$eventName code=$code';
}

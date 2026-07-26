import 'dart:convert';

import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_riverpod/misc.dart' show ProviderListenable;
import 'package:taskveil/src/core/bridge_service.dart';
import 'package:taskveil/src/rust/api.dart' show FrontendSettingKeyDto;
import 'package:taskveil/src/timer/timer_notifications.dart';

const timerSettingsKey = FrontendSettingKeyDto.timerSettings;

class TimerSettingsValidationException implements Exception {
  const TimerSettingsValidationException(this.field);

  final String field;

  @override
  String toString() => 'TimerSettingsValidationException($field)';
}

class TimerSettings {
  const TimerSettings({
    this.workMinutes = 25,
    this.shortBreakMinutes = 5,
    this.longBreakMinutes = 15,
    this.longBreakEvery = 4,
    this.notificationsEnabled = false,
  });

  final int workMinutes;
  final int shortBreakMinutes;
  final int longBreakMinutes;
  final int longBreakEvery;
  final bool notificationsEnabled;

  TimerSettings copyWith({
    int? workMinutes,
    int? shortBreakMinutes,
    int? longBreakMinutes,
    int? longBreakEvery,
    bool? notificationsEnabled,
  }) {
    return TimerSettings(
      workMinutes: workMinutes ?? this.workMinutes,
      shortBreakMinutes: shortBreakMinutes ?? this.shortBreakMinutes,
      longBreakMinutes: longBreakMinutes ?? this.longBreakMinutes,
      longBreakEvery: longBreakEvery ?? this.longBreakEvery,
      notificationsEnabled: notificationsEnabled ?? this.notificationsEnabled,
    );
  }

  TimerSettings validated() {
    _validateDuration(workMinutes, 'workMinutes', min: 5, max: 180);
    _validateDuration(shortBreakMinutes, 'shortBreakMinutes', min: 5, max: 60);
    _validateDuration(longBreakMinutes, 'longBreakMinutes', min: 5, max: 120);
    if (longBreakEvery < 2 || longBreakEvery > 12) {
      throw const TimerSettingsValidationException('longBreakEvery');
    }
    return this;
  }

  Map<String, Object> toJson() => {
    'version': 1,
    'workMinutes': workMinutes,
    'shortBreakMinutes': shortBreakMinutes,
    'longBreakMinutes': longBreakMinutes,
    'longBreakEvery': longBreakEvery,
    'notificationsEnabled': notificationsEnabled,
  };

  static TimerSettings fromJson(Object? value) {
    if (value is! Map<String, Object?> || value['version'] != 1) {
      throw const TimerSettingsValidationException('version');
    }
    final settings = TimerSettings(
      workMinutes: _readInt(value, 'workMinutes'),
      shortBreakMinutes: _readInt(value, 'shortBreakMinutes'),
      longBreakMinutes: _readInt(value, 'longBreakMinutes'),
      longBreakEvery: _readInt(value, 'longBreakEvery'),
      notificationsEnabled: value['notificationsEnabled'] == true,
    );
    return settings.validated();
  }

  static TimerSettings decode(String value) {
    try {
      return fromJson(jsonDecode(value));
    } on TimerSettingsValidationException {
      rethrow;
    } catch (_) {
      throw const TimerSettingsValidationException('json');
    }
  }

  String encode() => jsonEncode(toJson());

  static int _readInt(Map<String, Object?> value, String key) {
    final field = value[key];
    if (field is! int) {
      throw TimerSettingsValidationException(key);
    }
    return field;
  }

  static void _validateDuration(
    int value,
    String field, {
    required int min,
    required int max,
  }) {
    if (value < min || value > max || value % 5 != 0) {
      throw TimerSettingsValidationException(field);
    }
  }

  @override
  int get hashCode => Object.hash(
    workMinutes,
    shortBreakMinutes,
    longBreakMinutes,
    longBreakEvery,
    notificationsEnabled,
  );

  @override
  bool operator ==(Object other) =>
      other is TimerSettings &&
      workMinutes == other.workMinutes &&
      shortBreakMinutes == other.shortBreakMinutes &&
      longBreakMinutes == other.longBreakMinutes &&
      longBreakEvery == other.longBreakEvery &&
      notificationsEnabled == other.notificationsEnabled;
}

class TimerSettingsNotifier extends AsyncNotifier<TimerSettings> {
  TimerSettingsNotifier(this._bridgeProvider, this._notificationProvider);

  final ProviderListenable<BridgeService> _bridgeProvider;
  final ProviderListenable<TimerNotificationService> _notificationProvider;

  @override
  Future<TimerSettings> build() async {
    final persisted = await ref
        .watch(_bridgeProvider)
        .getFrontendSetting(key: timerSettingsKey);
    if (persisted == null) {
      return const TimerSettings();
    }
    try {
      return TimerSettings.decode(persisted);
    } on TimerSettingsValidationException {
      return const TimerSettings();
    }
  }

  Future<void> save(TimerSettings settings) async {
    var valid = settings.validated();
    final previous = state.value;
    if (valid.notificationsEnabled && previous?.notificationsEnabled != true) {
      final granted = await ref
          .read(_notificationProvider)
          .requestPermissions();
      if (!granted) {
        valid = valid.copyWith(notificationsEnabled: false);
      }
    }
    await ref
        .read(_bridgeProvider)
        .setFrontendSetting(key: timerSettingsKey, value: valid.encode());
    state = AsyncData(valid);
  }
}

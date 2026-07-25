import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:taskveil/src/billing/billing_store.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('RevenueCat build configuration', () {
    test('iOS selects only the iOS public SDK key', () {
      final configuration = resolveRevenueCatBuildConfiguration(
        platform: BillingStorePlatform.ios,
        requestedEnvironment: 'sandbox',
        buildEnvironment: 'sandbox',
        iosApiKey: 'ios_public_sdk_key',
        androidApiKey: 'android_public_sdk_key',
      );

      expect(configuration.apiKey, 'ios_public_sdk_key');
      expect(configuration.environment, 'sandbox');
    });

    test('Android selects only the Android public SDK key', () {
      final configuration = resolveRevenueCatBuildConfiguration(
        platform: BillingStorePlatform.android,
        requestedEnvironment: 'production',
        buildEnvironment: 'production',
        iosApiKey: 'ios_public_sdk_key',
        androidApiKey: 'android_public_sdk_key',
      );

      expect(configuration.apiKey, 'android_public_sdk_key');
      expect(configuration.environment, 'production');
    });

    test('Android does not fall back to the iOS public SDK key', () {
      expect(
        () => resolveRevenueCatBuildConfiguration(
          platform: BillingStorePlatform.android,
          requestedEnvironment: 'sandbox',
          buildEnvironment: 'sandbox',
          iosApiKey: 'ios_public_sdk_key',
          androidApiKey: '',
        ),
        throwsStateError,
      );
    });

    test('rejects a server and build environment mismatch', () {
      expect(
        () => resolveRevenueCatBuildConfiguration(
          platform: BillingStorePlatform.android,
          requestedEnvironment: 'production',
          buildEnvironment: 'sandbox',
          iosApiKey: 'ios_public_sdk_key',
          androidApiKey: 'android_public_sdk_key',
        ),
        throwsStateError,
      );
    });

    test('rejects an unknown build environment', () {
      expect(
        () => resolveRevenueCatBuildConfiguration(
          platform: BillingStorePlatform.ios,
          requestedEnvironment: 'development',
          buildEnvironment: 'development',
          iosApiKey: 'ios_public_sdk_key',
          androidApiKey: 'android_public_sdk_key',
        ),
        throwsStateError,
      );
    });

    test('desktop is explicitly unsupported', () {
      expect(
        () => resolveRevenueCatBuildConfiguration(
          platform: BillingStorePlatform.unsupported,
          requestedEnvironment: 'sandbox',
          buildEnvironment: 'sandbox',
          iosApiKey: 'ios_public_sdk_key',
          androidApiKey: 'android_public_sdk_key',
        ),
        throwsA(
          isA<UnsupportedError>().having(
            (error) => error.message,
            'message',
            contains('iOS and Android'),
          ),
        ),
      );
    });
  });

  group('RevenueCat store configuration', () {
    const channel = MethodChannel('purchases_flutter');

    for (final testCase in [
      (
        platform: BillingStorePlatform.ios,
        expectedApiKey: 'ios_public_sdk_key',
      ),
      (
        platform: BillingStorePlatform.android,
        expectedApiKey: 'android_public_sdk_key',
      ),
    ]) {
      test('${testCase.platform.name} configures its public SDK key', () async {
        MethodCall? setupCall;
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(channel, (call) async {
              if (call.method == 'isConfigured') return false;
              if (call.method == 'setupPurchases') {
                setupCall = call;
                return null;
              }
              throw MissingPluginException('Unexpected ${call.method}');
            });
        addTearDown(
          () => TestDefaultBinaryMessengerBinding
              .instance
              .defaultBinaryMessenger
              .setMockMethodCallHandler(channel, null),
        );
        final store = RevenueCatBillingStore(
          platform: testCase.platform,
          iosApiKey: 'ios_public_sdk_key',
          androidApiKey: 'android_public_sdk_key',
          environment: 'sandbox',
        );

        await store.configure(
          appUserId: '00000000-0000-4000-8000-000000000001',
          environment: 'sandbox',
        );

        expect(setupCall?.method, 'setupPurchases');
        final arguments = setupCall?.arguments as Map<Object?, Object?>;
        expect(arguments['apiKey'], testCase.expectedApiKey);
        expect(
          arguments['automaticDeviceIdentifierCollectionEnabled'],
          isFalse,
        );
      });
    }
  });
}

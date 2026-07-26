import 'dart:io';

import 'package:flutter/services.dart';
import 'package:purchases_flutter/purchases_flutter.dart';

const _revenueCatIosApiKey = String.fromEnvironment(
  'TASKVEIL_REVENUECAT_IOS_API_KEY',
);
const _revenueCatAndroidApiKey = String.fromEnvironment(
  'TASKVEIL_REVENUECAT_ANDROID_API_KEY',
);
const _revenueCatEnvironment = String.fromEnvironment(
  'TASKVEIL_REVENUECAT_ENVIRONMENT',
);

enum BillingPurchaseOutcome { purchased, cancelled, pending, failed }

enum BillingStorePlatform { ios, android, unsupported }

class RevenueCatBuildConfiguration {
  const RevenueCatBuildConfiguration({
    required this.apiKey,
    required this.environment,
  });

  final String apiKey;
  final String environment;
}

RevenueCatBuildConfiguration resolveRevenueCatBuildConfiguration({
  required BillingStorePlatform platform,
  required String requestedEnvironment,
  required String buildEnvironment,
  required String iosApiKey,
  required String androidApiKey,
}) {
  final apiKey = switch (platform) {
    BillingStorePlatform.ios => iosApiKey,
    BillingStorePlatform.android => androidApiKey,
    BillingStorePlatform.unsupported => throw UnsupportedError(
      'RevenueCat billing is supported only on iOS and Android',
    ),
  };
  if (apiKey.trim().isEmpty ||
      !const {'sandbox', 'production'}.contains(buildEnvironment) ||
      buildEnvironment != requestedEnvironment) {
    throw StateError('RevenueCat build configuration mismatch');
  }
  return RevenueCatBuildConfiguration(
    apiKey: apiKey,
    environment: buildEnvironment,
  );
}

class BillingProduct {
  const BillingProduct({
    required this.identifier,
    required this.title,
    required this.description,
    required this.price,
    required this.isAnnual,
  });

  final String identifier;
  final String title;
  final String description;
  final String price;
  final bool isAnnual;
}

abstract interface class BillingStore {
  Future<void> configure({
    required String appUserId,
    required String environment,
  });

  Future<List<BillingProduct>> products();

  Future<BillingPurchaseOutcome> purchase(String productIdentifier);

  Future<BillingPurchaseOutcome> restore();

  Future<Uri?> managementUrl();

  /// RevenueCat logout is intentionally not called because it creates an
  /// anonymous customer. The next Taskveil login switches to the server-issued
  /// custom App User ID with [Purchases.logIn].
  Future<void> accountLoggedOut();
}

enum _BillingAdmissionState { uninitialized, closed, open }

/// Serializes access to the process-wide billing SDK identity.
///
/// RevenueCat is a singleton SDK: `configure`/`logIn` from an obsolete
/// account generation can otherwise finish after the next account has already
/// configured the SDK. Every account-scoped action reasserts the server-issued
/// identity inside the same FIFO before touching the store.
class BillingStoreCoordinator {
  BillingStoreCoordinator(this._store);

  final BillingStore _store;
  Future<void> _tail = Future<void>.value();
  int _accountEpoch = 0;
  _BillingAdmissionState _admissionState = _BillingAdmissionState.uninitialized;
  (String, String)? _activeIdentity;

  int get accountEpoch => _accountEpoch;

  int closeAdmission() {
    _accountEpoch += 1;
    _admissionState = _BillingAdmissionState.closed;
    _activeIdentity = null;
    return _accountEpoch;
  }

  bool isCurrentEpoch(int accountEpoch) => accountEpoch == _accountEpoch;

  bool isOpenEpoch(int accountEpoch) =>
      isCurrentEpoch(accountEpoch) &&
      _admissionState == _BillingAdmissionState.open;

  void initializeAdmission({
    required int accountEpoch,
    required bool loggedIn,
  }) {
    if (!isCurrentEpoch(accountEpoch)) return;
    if (_admissionState == _BillingAdmissionState.uninitialized) {
      _admissionState = loggedIn
          ? _BillingAdmissionState.open
          : _BillingAdmissionState.closed;
    } else if (_admissionState == _BillingAdmissionState.open && !loggedIn) {
      closeAdmission();
    }
  }

  bool openAdmission(int accountEpoch) {
    if (!isCurrentEpoch(accountEpoch) ||
        _admissionState != _BillingAdmissionState.closed) {
      return false;
    }
    _accountEpoch += 1;
    _admissionState = _BillingAdmissionState.open;
    return true;
  }

  void closeAdmissionIfCurrent(int accountEpoch) {
    if (isCurrentEpoch(accountEpoch) &&
        _admissionState == _BillingAdmissionState.open) {
      closeAdmission();
    }
  }

  bool isAdmitted({
    required int accountEpoch,
    required String appUserId,
    required String environment,
  }) =>
      isOpenEpoch(accountEpoch) && _activeIdentity == (appUserId, environment);

  Future<List<BillingProduct>?> products({
    required int accountEpoch,
    required String appUserId,
    required String environment,
  }) => _serialize(() async {
    if (!isOpenEpoch(accountEpoch)) return null;
    await _store.configure(appUserId: appUserId, environment: environment);
    if (!isOpenEpoch(accountEpoch)) return null;
    _activeIdentity = (appUserId, environment);
    final products = await _store.products();
    return isAdmitted(
          accountEpoch: accountEpoch,
          appUserId: appUserId,
          environment: environment,
        )
        ? products
        : null;
  });

  Future<BillingPurchaseOutcome?> purchase({
    required int accountEpoch,
    required String appUserId,
    required String environment,
    required String productIdentifier,
  }) => _serialize(() async {
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    await _store.configure(appUserId: appUserId, environment: environment);
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    return _store.purchase(productIdentifier);
  });

  Future<BillingPurchaseOutcome?> restore({
    required int accountEpoch,
    required String appUserId,
    required String environment,
  }) => _serialize(() async {
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    await _store.configure(appUserId: appUserId, environment: environment);
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    return _store.restore();
  });

  Future<Uri?> managementUrl({
    required int accountEpoch,
    required String appUserId,
    required String environment,
  }) => _serialize(() async {
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    await _store.configure(appUserId: appUserId, environment: environment);
    if (!isAdmitted(
      accountEpoch: accountEpoch,
      appUserId: appUserId,
      environment: environment,
    )) {
      return null;
    }
    return _store.managementUrl();
  });

  Future<void> accountLoggedOut({required int accountEpoch}) =>
      _serialize(() async {
        if (!isCurrentEpoch(accountEpoch) ||
            _admissionState != _BillingAdmissionState.closed) {
          return;
        }
        await _store.accountLoggedOut();
      });

  Future<bool> drainClosedAdmission({required int accountEpoch}) => _serialize(
    () async =>
        isCurrentEpoch(accountEpoch) &&
        _admissionState == _BillingAdmissionState.closed,
  );

  Future<T> _serialize<T>(Future<T> Function() operation) {
    final scheduled = _tail.then((_) => operation());
    _tail = scheduled.then<void>((_) {}, onError: (_, _) {});
    return scheduled;
  }
}

class RevenueCatBillingStore implements BillingStore {
  RevenueCatBillingStore({
    BillingStorePlatform? platform,
    String? iosApiKey,
    String? androidApiKey,
    String? environment,
  }) : _platform = platform ?? _currentPlatform(),
       _iosApiKey = iosApiKey ?? _revenueCatIosApiKey,
       _androidApiKey = androidApiKey ?? _revenueCatAndroidApiKey,
       _environment = environment ?? _revenueCatEnvironment;

  final BillingStorePlatform _platform;
  final String _iosApiKey;
  final String _androidApiKey;
  final String _environment;
  Package? _monthly;
  Package? _annual;

  @override
  Future<void> configure({
    required String appUserId,
    required String environment,
  }) async {
    final buildConfiguration = _buildConfiguration(environment);
    if (await Purchases.isConfigured) {
      if (await Purchases.appUserID != appUserId) {
        await Purchases.logIn(appUserId);
      }
      return;
    }
    final configuration = PurchasesConfiguration(buildConfiguration.apiKey)
      ..appUserID = appUserId
      ..automaticDeviceIdentifierCollectionEnabled = false;
    await Purchases.configure(configuration);
  }

  @override
  Future<List<BillingProduct>> products() async {
    _ensureSupportedPlatform();
    final offering = (await Purchases.getOfferings()).getOffering('default');
    if (offering == null) return const [];
    _monthly = offering.availablePackages.where(_isMonthly).firstOrNull;
    _annual = offering.availablePackages.where(_isAnnual).firstOrNull;
    return [
      if (_monthly case final package?) _product(package, isAnnual: false),
      if (_annual case final package?) _product(package, isAnnual: true),
    ];
  }

  @override
  Future<BillingPurchaseOutcome> purchase(String productIdentifier) async {
    _ensureSupportedPlatform();
    final package = [_monthly, _annual]
        .whereType<Package>()
        .where(
          (candidate) => candidate.storeProduct.identifier == productIdentifier,
        )
        .firstOrNull;
    if (package == null) return BillingPurchaseOutcome.failed;
    try {
      await Purchases.purchase(PurchaseParams.package(package));
      return BillingPurchaseOutcome.purchased;
    } on PlatformException catch (error) {
      return _purchaseError(error);
    }
  }

  @override
  Future<BillingPurchaseOutcome> restore() async {
    _ensureSupportedPlatform();
    try {
      await Purchases.restorePurchases();
      return BillingPurchaseOutcome.purchased;
    } on PlatformException catch (error) {
      return _purchaseError(error);
    }
  }

  @override
  Future<Uri?> managementUrl() async {
    _ensureSupportedPlatform();
    final value = (await Purchases.getCustomerInfo()).managementURL;
    return value == null ? null : Uri.tryParse(value);
  }

  @override
  Future<void> accountLoggedOut() async {
    _monthly = null;
    _annual = null;
  }

  RevenueCatBuildConfiguration _buildConfiguration(
    String requestedEnvironment,
  ) => resolveRevenueCatBuildConfiguration(
    platform: _platform,
    requestedEnvironment: requestedEnvironment,
    buildEnvironment: _environment,
    iosApiKey: _iosApiKey,
    androidApiKey: _androidApiKey,
  );

  void _ensureSupportedPlatform() {
    if (_platform == BillingStorePlatform.unsupported) {
      throw UnsupportedError(
        'RevenueCat billing is supported only on iOS and Android',
      );
    }
  }

  static BillingStorePlatform _currentPlatform() {
    if (Platform.isIOS) return BillingStorePlatform.ios;
    if (Platform.isAndroid) return BillingStorePlatform.android;
    return BillingStorePlatform.unsupported;
  }

  static BillingProduct _product(Package package, {required bool isAnnual}) {
    final product = package.storeProduct;
    return BillingProduct(
      identifier: product.identifier,
      title: product.title,
      description: product.description,
      price: product.priceString,
      isAnnual: isAnnual,
    );
  }

  static bool _isMonthly(Package package) =>
      package.storeProduct.identifier == 'com.taskveil.app.pro.monthly';

  static bool _isAnnual(Package package) =>
      package.storeProduct.identifier == 'com.taskveil.app.pro.yearly';

  static BillingPurchaseOutcome _purchaseError(PlatformException error) {
    return switch (PurchasesErrorHelper.getErrorCode(error)) {
      PurchasesErrorCode.purchaseCancelledError =>
        BillingPurchaseOutcome.cancelled,
      PurchasesErrorCode.paymentPendingError => BillingPurchaseOutcome.pending,
      _ => BillingPurchaseOutcome.failed,
    };
  }
}

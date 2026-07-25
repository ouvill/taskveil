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

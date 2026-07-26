import 'package:timezone/timezone.dart' as tz;

/// Returns local midnight at [dayOffset] civil days from [value].
///
/// A civil day is advanced through calendar fields rather than elapsed time,
/// so the result remains midnight when a timezone changes its UTC offset.
/// Explicit non-UTC [tz.TZDateTime] inputs keep their named location, which
/// also provides a deterministic test seam independent of the host timezone.
DateTime localCivilDay(DateTime value, {int dayOffset = 0}) {
  final local = value is tz.TZDateTime && !value.isUtc
      ? value
      : value.toLocal();
  final day = local.day + dayOffset;
  if (local is tz.TZDateTime) {
    return tz.TZDateTime(local.location, local.year, local.month, day);
  }
  return DateTime(local.year, local.month, day);
}

// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'api.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$CalendarOccurrenceKindDto {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CalendarOccurrenceKindDto);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'CalendarOccurrenceKindDto()';
}


}

/// @nodoc
class $CalendarOccurrenceKindDtoCopyWith<$Res>  {
$CalendarOccurrenceKindDtoCopyWith(CalendarOccurrenceKindDto _, $Res Function(CalendarOccurrenceKindDto) __);
}


/// Adds pattern-matching-related methods to [CalendarOccurrenceKindDto].
extension CalendarOccurrenceKindDtoPatterns on CalendarOccurrenceKindDto {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( CalendarOccurrenceKindDto_DateDue value)?  dateDue,TResult Function( CalendarOccurrenceKindDto_DateTimeDue value)?  dateTimeDue,TResult Function( CalendarOccurrenceKindDto_Scheduled value)?  scheduled,TResult Function( CalendarOccurrenceKindDto_Completed value)?  completed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue() when dateDue != null:
return dateDue(_that);case CalendarOccurrenceKindDto_DateTimeDue() when dateTimeDue != null:
return dateTimeDue(_that);case CalendarOccurrenceKindDto_Scheduled() when scheduled != null:
return scheduled(_that);case CalendarOccurrenceKindDto_Completed() when completed != null:
return completed(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( CalendarOccurrenceKindDto_DateDue value)  dateDue,required TResult Function( CalendarOccurrenceKindDto_DateTimeDue value)  dateTimeDue,required TResult Function( CalendarOccurrenceKindDto_Scheduled value)  scheduled,required TResult Function( CalendarOccurrenceKindDto_Completed value)  completed,}){
final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue():
return dateDue(_that);case CalendarOccurrenceKindDto_DateTimeDue():
return dateTimeDue(_that);case CalendarOccurrenceKindDto_Scheduled():
return scheduled(_that);case CalendarOccurrenceKindDto_Completed():
return completed(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( CalendarOccurrenceKindDto_DateDue value)?  dateDue,TResult? Function( CalendarOccurrenceKindDto_DateTimeDue value)?  dateTimeDue,TResult? Function( CalendarOccurrenceKindDto_Scheduled value)?  scheduled,TResult? Function( CalendarOccurrenceKindDto_Completed value)?  completed,}){
final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue() when dateDue != null:
return dateDue(_that);case CalendarOccurrenceKindDto_DateTimeDue() when dateTimeDue != null:
return dateTimeDue(_that);case CalendarOccurrenceKindDto_Scheduled() when scheduled != null:
return scheduled(_that);case CalendarOccurrenceKindDto_Completed() when completed != null:
return completed(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String dueOn)?  dateDue,TResult Function( DateTime dueAt,  String timeZone)?  dateTimeDue,TResult Function( DateTime scheduledAt)?  scheduled,TResult Function( DateTime completedAt)?  completed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue() when dateDue != null:
return dateDue(_that.dueOn);case CalendarOccurrenceKindDto_DateTimeDue() when dateTimeDue != null:
return dateTimeDue(_that.dueAt,_that.timeZone);case CalendarOccurrenceKindDto_Scheduled() when scheduled != null:
return scheduled(_that.scheduledAt);case CalendarOccurrenceKindDto_Completed() when completed != null:
return completed(_that.completedAt);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String dueOn)  dateDue,required TResult Function( DateTime dueAt,  String timeZone)  dateTimeDue,required TResult Function( DateTime scheduledAt)  scheduled,required TResult Function( DateTime completedAt)  completed,}) {final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue():
return dateDue(_that.dueOn);case CalendarOccurrenceKindDto_DateTimeDue():
return dateTimeDue(_that.dueAt,_that.timeZone);case CalendarOccurrenceKindDto_Scheduled():
return scheduled(_that.scheduledAt);case CalendarOccurrenceKindDto_Completed():
return completed(_that.completedAt);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String dueOn)?  dateDue,TResult? Function( DateTime dueAt,  String timeZone)?  dateTimeDue,TResult? Function( DateTime scheduledAt)?  scheduled,TResult? Function( DateTime completedAt)?  completed,}) {final _that = this;
switch (_that) {
case CalendarOccurrenceKindDto_DateDue() when dateDue != null:
return dateDue(_that.dueOn);case CalendarOccurrenceKindDto_DateTimeDue() when dateTimeDue != null:
return dateTimeDue(_that.dueAt,_that.timeZone);case CalendarOccurrenceKindDto_Scheduled() when scheduled != null:
return scheduled(_that.scheduledAt);case CalendarOccurrenceKindDto_Completed() when completed != null:
return completed(_that.completedAt);case _:
  return null;

}
}

}

/// @nodoc


class CalendarOccurrenceKindDto_DateDue extends CalendarOccurrenceKindDto {
  const CalendarOccurrenceKindDto_DateDue({required this.dueOn}): super._();


 final  String dueOn;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CalendarOccurrenceKindDto_DateDueCopyWith<CalendarOccurrenceKindDto_DateDue> get copyWith => _$CalendarOccurrenceKindDto_DateDueCopyWithImpl<CalendarOccurrenceKindDto_DateDue>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CalendarOccurrenceKindDto_DateDue&&(identical(other.dueOn, dueOn) || other.dueOn == dueOn));
}


@override
int get hashCode => Object.hash(runtimeType,dueOn);

@override
String toString() {
  return 'CalendarOccurrenceKindDto.dateDue(dueOn: $dueOn)';
}


}

/// @nodoc
abstract mixin class $CalendarOccurrenceKindDto_DateDueCopyWith<$Res> implements $CalendarOccurrenceKindDtoCopyWith<$Res> {
  factory $CalendarOccurrenceKindDto_DateDueCopyWith(CalendarOccurrenceKindDto_DateDue value, $Res Function(CalendarOccurrenceKindDto_DateDue) _then) = _$CalendarOccurrenceKindDto_DateDueCopyWithImpl;
@useResult
$Res call({
 String dueOn
});




}
/// @nodoc
class _$CalendarOccurrenceKindDto_DateDueCopyWithImpl<$Res>
    implements $CalendarOccurrenceKindDto_DateDueCopyWith<$Res> {
  _$CalendarOccurrenceKindDto_DateDueCopyWithImpl(this._self, this._then);

  final CalendarOccurrenceKindDto_DateDue _self;
  final $Res Function(CalendarOccurrenceKindDto_DateDue) _then;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueOn = null,}) {
  return _then(CalendarOccurrenceKindDto_DateDue(
dueOn: null == dueOn ? _self.dueOn : dueOn // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CalendarOccurrenceKindDto_DateTimeDue extends CalendarOccurrenceKindDto {
  const CalendarOccurrenceKindDto_DateTimeDue({required this.dueAt, required this.timeZone}): super._();


 final  DateTime dueAt;
 final  String timeZone;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CalendarOccurrenceKindDto_DateTimeDueCopyWith<CalendarOccurrenceKindDto_DateTimeDue> get copyWith => _$CalendarOccurrenceKindDto_DateTimeDueCopyWithImpl<CalendarOccurrenceKindDto_DateTimeDue>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CalendarOccurrenceKindDto_DateTimeDue&&(identical(other.dueAt, dueAt) || other.dueAt == dueAt)&&(identical(other.timeZone, timeZone) || other.timeZone == timeZone));
}


@override
int get hashCode => Object.hash(runtimeType,dueAt,timeZone);

@override
String toString() {
  return 'CalendarOccurrenceKindDto.dateTimeDue(dueAt: $dueAt, timeZone: $timeZone)';
}


}

/// @nodoc
abstract mixin class $CalendarOccurrenceKindDto_DateTimeDueCopyWith<$Res> implements $CalendarOccurrenceKindDtoCopyWith<$Res> {
  factory $CalendarOccurrenceKindDto_DateTimeDueCopyWith(CalendarOccurrenceKindDto_DateTimeDue value, $Res Function(CalendarOccurrenceKindDto_DateTimeDue) _then) = _$CalendarOccurrenceKindDto_DateTimeDueCopyWithImpl;
@useResult
$Res call({
 DateTime dueAt, String timeZone
});




}
/// @nodoc
class _$CalendarOccurrenceKindDto_DateTimeDueCopyWithImpl<$Res>
    implements $CalendarOccurrenceKindDto_DateTimeDueCopyWith<$Res> {
  _$CalendarOccurrenceKindDto_DateTimeDueCopyWithImpl(this._self, this._then);

  final CalendarOccurrenceKindDto_DateTimeDue _self;
  final $Res Function(CalendarOccurrenceKindDto_DateTimeDue) _then;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueAt = null,Object? timeZone = null,}) {
  return _then(CalendarOccurrenceKindDto_DateTimeDue(
dueAt: null == dueAt ? _self.dueAt : dueAt // ignore: cast_nullable_to_non_nullable
as DateTime,timeZone: null == timeZone ? _self.timeZone : timeZone // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class CalendarOccurrenceKindDto_Scheduled extends CalendarOccurrenceKindDto {
  const CalendarOccurrenceKindDto_Scheduled({required this.scheduledAt}): super._();


 final  DateTime scheduledAt;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CalendarOccurrenceKindDto_ScheduledCopyWith<CalendarOccurrenceKindDto_Scheduled> get copyWith => _$CalendarOccurrenceKindDto_ScheduledCopyWithImpl<CalendarOccurrenceKindDto_Scheduled>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CalendarOccurrenceKindDto_Scheduled&&(identical(other.scheduledAt, scheduledAt) || other.scheduledAt == scheduledAt));
}


@override
int get hashCode => Object.hash(runtimeType,scheduledAt);

@override
String toString() {
  return 'CalendarOccurrenceKindDto.scheduled(scheduledAt: $scheduledAt)';
}


}

/// @nodoc
abstract mixin class $CalendarOccurrenceKindDto_ScheduledCopyWith<$Res> implements $CalendarOccurrenceKindDtoCopyWith<$Res> {
  factory $CalendarOccurrenceKindDto_ScheduledCopyWith(CalendarOccurrenceKindDto_Scheduled value, $Res Function(CalendarOccurrenceKindDto_Scheduled) _then) = _$CalendarOccurrenceKindDto_ScheduledCopyWithImpl;
@useResult
$Res call({
 DateTime scheduledAt
});




}
/// @nodoc
class _$CalendarOccurrenceKindDto_ScheduledCopyWithImpl<$Res>
    implements $CalendarOccurrenceKindDto_ScheduledCopyWith<$Res> {
  _$CalendarOccurrenceKindDto_ScheduledCopyWithImpl(this._self, this._then);

  final CalendarOccurrenceKindDto_Scheduled _self;
  final $Res Function(CalendarOccurrenceKindDto_Scheduled) _then;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? scheduledAt = null,}) {
  return _then(CalendarOccurrenceKindDto_Scheduled(
scheduledAt: null == scheduledAt ? _self.scheduledAt : scheduledAt // ignore: cast_nullable_to_non_nullable
as DateTime,
  ));
}


}

/// @nodoc


class CalendarOccurrenceKindDto_Completed extends CalendarOccurrenceKindDto {
  const CalendarOccurrenceKindDto_Completed({required this.completedAt}): super._();


 final  DateTime completedAt;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CalendarOccurrenceKindDto_CompletedCopyWith<CalendarOccurrenceKindDto_Completed> get copyWith => _$CalendarOccurrenceKindDto_CompletedCopyWithImpl<CalendarOccurrenceKindDto_Completed>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is CalendarOccurrenceKindDto_Completed&&(identical(other.completedAt, completedAt) || other.completedAt == completedAt));
}


@override
int get hashCode => Object.hash(runtimeType,completedAt);

@override
String toString() {
  return 'CalendarOccurrenceKindDto.completed(completedAt: $completedAt)';
}


}

/// @nodoc
abstract mixin class $CalendarOccurrenceKindDto_CompletedCopyWith<$Res> implements $CalendarOccurrenceKindDtoCopyWith<$Res> {
  factory $CalendarOccurrenceKindDto_CompletedCopyWith(CalendarOccurrenceKindDto_Completed value, $Res Function(CalendarOccurrenceKindDto_Completed) _then) = _$CalendarOccurrenceKindDto_CompletedCopyWithImpl;
@useResult
$Res call({
 DateTime completedAt
});




}
/// @nodoc
class _$CalendarOccurrenceKindDto_CompletedCopyWithImpl<$Res>
    implements $CalendarOccurrenceKindDto_CompletedCopyWith<$Res> {
  _$CalendarOccurrenceKindDto_CompletedCopyWithImpl(this._self, this._then);

  final CalendarOccurrenceKindDto_Completed _self;
  final $Res Function(CalendarOccurrenceKindDto_Completed) _then;

/// Create a copy of CalendarOccurrenceKindDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? completedAt = null,}) {
  return _then(CalendarOccurrenceKindDto_Completed(
completedAt: null == completedAt ? _self.completedAt : completedAt // ignore: cast_nullable_to_non_nullable
as DateTime,
  ));
}


}

/// @nodoc
mixin _$SyncNowOutcomeDto {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SyncNowOutcomeDto);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'SyncNowOutcomeDto()';
}


}

/// @nodoc
class $SyncNowOutcomeDtoCopyWith<$Res>  {
$SyncNowOutcomeDtoCopyWith(SyncNowOutcomeDto _, $Res Function(SyncNowOutcomeDto) __);
}


/// Adds pattern-matching-related methods to [SyncNowOutcomeDto].
extension SyncNowOutcomeDtoPatterns on SyncNowOutcomeDto {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( SyncNowOutcomeDto_Synced value)?  synced,TResult Function( SyncNowOutcomeDto_BillingRequired value)?  billingRequired,required TResult orElse(),}){
final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced() when synced != null:
return synced(_that);case SyncNowOutcomeDto_BillingRequired() when billingRequired != null:
return billingRequired(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( SyncNowOutcomeDto_Synced value)  synced,required TResult Function( SyncNowOutcomeDto_BillingRequired value)  billingRequired,}){
final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced():
return synced(_that);case SyncNowOutcomeDto_BillingRequired():
return billingRequired(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( SyncNowOutcomeDto_Synced value)?  synced,TResult? Function( SyncNowOutcomeDto_BillingRequired value)?  billingRequired,}){
final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced() when synced != null:
return synced(_that);case SyncNowOutcomeDto_BillingRequired() when billingRequired != null:
return billingRequired(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( SyncStatusDto status)?  synced,TResult Function()?  billingRequired,required TResult orElse(),}) {final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced() when synced != null:
return synced(_that.status);case SyncNowOutcomeDto_BillingRequired() when billingRequired != null:
return billingRequired();case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( SyncStatusDto status)  synced,required TResult Function()  billingRequired,}) {final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced():
return synced(_that.status);case SyncNowOutcomeDto_BillingRequired():
return billingRequired();}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( SyncStatusDto status)?  synced,TResult? Function()?  billingRequired,}) {final _that = this;
switch (_that) {
case SyncNowOutcomeDto_Synced() when synced != null:
return synced(_that.status);case SyncNowOutcomeDto_BillingRequired() when billingRequired != null:
return billingRequired();case _:
  return null;

}
}

}

/// @nodoc


class SyncNowOutcomeDto_Synced extends SyncNowOutcomeDto {
  const SyncNowOutcomeDto_Synced({required this.status}): super._();


 final  SyncStatusDto status;

/// Create a copy of SyncNowOutcomeDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SyncNowOutcomeDto_SyncedCopyWith<SyncNowOutcomeDto_Synced> get copyWith => _$SyncNowOutcomeDto_SyncedCopyWithImpl<SyncNowOutcomeDto_Synced>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SyncNowOutcomeDto_Synced&&(identical(other.status, status) || other.status == status));
}


@override
int get hashCode => Object.hash(runtimeType,status);

@override
String toString() {
  return 'SyncNowOutcomeDto.synced(status: $status)';
}


}

/// @nodoc
abstract mixin class $SyncNowOutcomeDto_SyncedCopyWith<$Res> implements $SyncNowOutcomeDtoCopyWith<$Res> {
  factory $SyncNowOutcomeDto_SyncedCopyWith(SyncNowOutcomeDto_Synced value, $Res Function(SyncNowOutcomeDto_Synced) _then) = _$SyncNowOutcomeDto_SyncedCopyWithImpl;
@useResult
$Res call({
 SyncStatusDto status
});




}
/// @nodoc
class _$SyncNowOutcomeDto_SyncedCopyWithImpl<$Res>
    implements $SyncNowOutcomeDto_SyncedCopyWith<$Res> {
  _$SyncNowOutcomeDto_SyncedCopyWithImpl(this._self, this._then);

  final SyncNowOutcomeDto_Synced _self;
  final $Res Function(SyncNowOutcomeDto_Synced) _then;

/// Create a copy of SyncNowOutcomeDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? status = null,}) {
  return _then(SyncNowOutcomeDto_Synced(
status: null == status ? _self.status : status // ignore: cast_nullable_to_non_nullable
as SyncStatusDto,
  ));
}


}

/// @nodoc


class SyncNowOutcomeDto_BillingRequired extends SyncNowOutcomeDto {
  const SyncNowOutcomeDto_BillingRequired(): super._();







@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is SyncNowOutcomeDto_BillingRequired);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'SyncNowOutcomeDto.billingRequired()';
}


}




/// @nodoc
mixin _$TaskDueDto {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueDto);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TaskDueDto()';
}


}

/// @nodoc
class $TaskDueDtoCopyWith<$Res>  {
$TaskDueDtoCopyWith(TaskDueDto _, $Res Function(TaskDueDto) __);
}


/// Adds pattern-matching-related methods to [TaskDueDto].
extension TaskDueDtoPatterns on TaskDueDto {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( TaskDueDto_Date value)?  date,TResult Function( TaskDueDto_DateTime value)?  dateTime,required TResult orElse(),}){
final _that = this;
switch (_that) {
case TaskDueDto_Date() when date != null:
return date(_that);case TaskDueDto_DateTime() when dateTime != null:
return dateTime(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( TaskDueDto_Date value)  date,required TResult Function( TaskDueDto_DateTime value)  dateTime,}){
final _that = this;
switch (_that) {
case TaskDueDto_Date():
return date(_that);case TaskDueDto_DateTime():
return dateTime(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( TaskDueDto_Date value)?  date,TResult? Function( TaskDueDto_DateTime value)?  dateTime,}){
final _that = this;
switch (_that) {
case TaskDueDto_Date() when date != null:
return date(_that);case TaskDueDto_DateTime() when dateTime != null:
return dateTime(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String dueOn)?  date,TResult Function( DateTime dueAt,  String timeZone)?  dateTime,required TResult orElse(),}) {final _that = this;
switch (_that) {
case TaskDueDto_Date() when date != null:
return date(_that.dueOn);case TaskDueDto_DateTime() when dateTime != null:
return dateTime(_that.dueAt,_that.timeZone);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String dueOn)  date,required TResult Function( DateTime dueAt,  String timeZone)  dateTime,}) {final _that = this;
switch (_that) {
case TaskDueDto_Date():
return date(_that.dueOn);case TaskDueDto_DateTime():
return dateTime(_that.dueAt,_that.timeZone);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String dueOn)?  date,TResult? Function( DateTime dueAt,  String timeZone)?  dateTime,}) {final _that = this;
switch (_that) {
case TaskDueDto_Date() when date != null:
return date(_that.dueOn);case TaskDueDto_DateTime() when dateTime != null:
return dateTime(_that.dueAt,_that.timeZone);case _:
  return null;

}
}

}

/// @nodoc


class TaskDueDto_Date extends TaskDueDto {
  const TaskDueDto_Date({required this.dueOn}): super._();


 final  String dueOn;

/// Create a copy of TaskDueDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TaskDueDto_DateCopyWith<TaskDueDto_Date> get copyWith => _$TaskDueDto_DateCopyWithImpl<TaskDueDto_Date>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueDto_Date&&(identical(other.dueOn, dueOn) || other.dueOn == dueOn));
}


@override
int get hashCode => Object.hash(runtimeType,dueOn);

@override
String toString() {
  return 'TaskDueDto.date(dueOn: $dueOn)';
}


}

/// @nodoc
abstract mixin class $TaskDueDto_DateCopyWith<$Res> implements $TaskDueDtoCopyWith<$Res> {
  factory $TaskDueDto_DateCopyWith(TaskDueDto_Date value, $Res Function(TaskDueDto_Date) _then) = _$TaskDueDto_DateCopyWithImpl;
@useResult
$Res call({
 String dueOn
});




}
/// @nodoc
class _$TaskDueDto_DateCopyWithImpl<$Res>
    implements $TaskDueDto_DateCopyWith<$Res> {
  _$TaskDueDto_DateCopyWithImpl(this._self, this._then);

  final TaskDueDto_Date _self;
  final $Res Function(TaskDueDto_Date) _then;

/// Create a copy of TaskDueDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueOn = null,}) {
  return _then(TaskDueDto_Date(
dueOn: null == dueOn ? _self.dueOn : dueOn // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class TaskDueDto_DateTime extends TaskDueDto {
  const TaskDueDto_DateTime({required this.dueAt, required this.timeZone}): super._();


 final  DateTime dueAt;
 final  String timeZone;

/// Create a copy of TaskDueDto
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TaskDueDto_DateTimeCopyWith<TaskDueDto_DateTime> get copyWith => _$TaskDueDto_DateTimeCopyWithImpl<TaskDueDto_DateTime>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueDto_DateTime&&(identical(other.dueAt, dueAt) || other.dueAt == dueAt)&&(identical(other.timeZone, timeZone) || other.timeZone == timeZone));
}


@override
int get hashCode => Object.hash(runtimeType,dueAt,timeZone);

@override
String toString() {
  return 'TaskDueDto.dateTime(dueAt: $dueAt, timeZone: $timeZone)';
}


}

/// @nodoc
abstract mixin class $TaskDueDto_DateTimeCopyWith<$Res> implements $TaskDueDtoCopyWith<$Res> {
  factory $TaskDueDto_DateTimeCopyWith(TaskDueDto_DateTime value, $Res Function(TaskDueDto_DateTime) _then) = _$TaskDueDto_DateTimeCopyWithImpl;
@useResult
$Res call({
 DateTime dueAt, String timeZone
});




}
/// @nodoc
class _$TaskDueDto_DateTimeCopyWithImpl<$Res>
    implements $TaskDueDto_DateTimeCopyWith<$Res> {
  _$TaskDueDto_DateTimeCopyWithImpl(this._self, this._then);

  final TaskDueDto_DateTime _self;
  final $Res Function(TaskDueDto_DateTime) _then;

/// Create a copy of TaskDueDto
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueAt = null,Object? timeZone = null,}) {
  return _then(TaskDueDto_DateTime(
dueAt: null == dueAt ? _self.dueAt : dueAt // ignore: cast_nullable_to_non_nullable
as DateTime,timeZone: null == timeZone ? _self.timeZone : timeZone // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$TaskDueInput {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueInput);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'TaskDueInput()';
}


}

/// @nodoc
class $TaskDueInputCopyWith<$Res>  {
$TaskDueInputCopyWith(TaskDueInput _, $Res Function(TaskDueInput) __);
}


/// Adds pattern-matching-related methods to [TaskDueInput].
extension TaskDueInputPatterns on TaskDueInput {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( TaskDueInput_Date value)?  date,TResult Function( TaskDueInput_DateTime value)?  dateTime,required TResult orElse(),}){
final _that = this;
switch (_that) {
case TaskDueInput_Date() when date != null:
return date(_that);case TaskDueInput_DateTime() when dateTime != null:
return dateTime(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( TaskDueInput_Date value)  date,required TResult Function( TaskDueInput_DateTime value)  dateTime,}){
final _that = this;
switch (_that) {
case TaskDueInput_Date():
return date(_that);case TaskDueInput_DateTime():
return dateTime(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( TaskDueInput_Date value)?  date,TResult? Function( TaskDueInput_DateTime value)?  dateTime,}){
final _that = this;
switch (_that) {
case TaskDueInput_Date() when date != null:
return date(_that);case TaskDueInput_DateTime() when dateTime != null:
return dateTime(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String dueOn)?  date,TResult Function( DateTime dueAt,  String timeZone)?  dateTime,required TResult orElse(),}) {final _that = this;
switch (_that) {
case TaskDueInput_Date() when date != null:
return date(_that.dueOn);case TaskDueInput_DateTime() when dateTime != null:
return dateTime(_that.dueAt,_that.timeZone);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String dueOn)  date,required TResult Function( DateTime dueAt,  String timeZone)  dateTime,}) {final _that = this;
switch (_that) {
case TaskDueInput_Date():
return date(_that.dueOn);case TaskDueInput_DateTime():
return dateTime(_that.dueAt,_that.timeZone);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String dueOn)?  date,TResult? Function( DateTime dueAt,  String timeZone)?  dateTime,}) {final _that = this;
switch (_that) {
case TaskDueInput_Date() when date != null:
return date(_that.dueOn);case TaskDueInput_DateTime() when dateTime != null:
return dateTime(_that.dueAt,_that.timeZone);case _:
  return null;

}
}

}

/// @nodoc


class TaskDueInput_Date extends TaskDueInput {
  const TaskDueInput_Date({required this.dueOn}): super._();


 final  String dueOn;

/// Create a copy of TaskDueInput
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TaskDueInput_DateCopyWith<TaskDueInput_Date> get copyWith => _$TaskDueInput_DateCopyWithImpl<TaskDueInput_Date>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueInput_Date&&(identical(other.dueOn, dueOn) || other.dueOn == dueOn));
}


@override
int get hashCode => Object.hash(runtimeType,dueOn);

@override
String toString() {
  return 'TaskDueInput.date(dueOn: $dueOn)';
}


}

/// @nodoc
abstract mixin class $TaskDueInput_DateCopyWith<$Res> implements $TaskDueInputCopyWith<$Res> {
  factory $TaskDueInput_DateCopyWith(TaskDueInput_Date value, $Res Function(TaskDueInput_Date) _then) = _$TaskDueInput_DateCopyWithImpl;
@useResult
$Res call({
 String dueOn
});




}
/// @nodoc
class _$TaskDueInput_DateCopyWithImpl<$Res>
    implements $TaskDueInput_DateCopyWith<$Res> {
  _$TaskDueInput_DateCopyWithImpl(this._self, this._then);

  final TaskDueInput_Date _self;
  final $Res Function(TaskDueInput_Date) _then;

/// Create a copy of TaskDueInput
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueOn = null,}) {
  return _then(TaskDueInput_Date(
dueOn: null == dueOn ? _self.dueOn : dueOn // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class TaskDueInput_DateTime extends TaskDueInput {
  const TaskDueInput_DateTime({required this.dueAt, required this.timeZone}): super._();


 final  DateTime dueAt;
 final  String timeZone;

/// Create a copy of TaskDueInput
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TaskDueInput_DateTimeCopyWith<TaskDueInput_DateTime> get copyWith => _$TaskDueInput_DateTimeCopyWithImpl<TaskDueInput_DateTime>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TaskDueInput_DateTime&&(identical(other.dueAt, dueAt) || other.dueAt == dueAt)&&(identical(other.timeZone, timeZone) || other.timeZone == timeZone));
}


@override
int get hashCode => Object.hash(runtimeType,dueAt,timeZone);

@override
String toString() {
  return 'TaskDueInput.dateTime(dueAt: $dueAt, timeZone: $timeZone)';
}


}

/// @nodoc
abstract mixin class $TaskDueInput_DateTimeCopyWith<$Res> implements $TaskDueInputCopyWith<$Res> {
  factory $TaskDueInput_DateTimeCopyWith(TaskDueInput_DateTime value, $Res Function(TaskDueInput_DateTime) _then) = _$TaskDueInput_DateTimeCopyWithImpl;
@useResult
$Res call({
 DateTime dueAt, String timeZone
});




}
/// @nodoc
class _$TaskDueInput_DateTimeCopyWithImpl<$Res>
    implements $TaskDueInput_DateTimeCopyWith<$Res> {
  _$TaskDueInput_DateTimeCopyWithImpl(this._self, this._then);

  final TaskDueInput_DateTime _self;
  final $Res Function(TaskDueInput_DateTime) _then;

/// Create a copy of TaskDueInput
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? dueAt = null,Object? timeZone = null,}) {
  return _then(TaskDueInput_DateTime(
dueAt: null == dueAt ? _self.dueAt : dueAt // ignore: cast_nullable_to_non_nullable
as DateTime,timeZone: null == timeZone ? _self.timeZone : timeZone // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on

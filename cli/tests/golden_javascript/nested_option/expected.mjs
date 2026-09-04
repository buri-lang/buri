const $k0=[1n,void 0,3n];
const $k1=[2n];
const $k2=[void 0];
const $k3=[1n];
const $k4=[9n];
const $k5=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
$D0.push(2,'Holder',true,['inner'],[$D1]);
$D1.push(7,$D2);
$D2.push(0,'I');
function $eqD0(a,b){
  if(a===b){
    return true;
  }
  return $eq(a[0],b[0]);
}
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const ss_1=$some(7n);
  const sn_2=$some(void 0);
  const nn_3=void 0;
  let $t1;
  if(ss_1!==void 0&&$val(ss_1)!==void 0){
    $val(ss_1);
    $t1='some some';
  }else if(ss_1!==void 0&&$val(ss_1)===void 0){
    $t1='some none';
  }else if(ss_1===void 0){
    $t1='none';
  }else{
    $abort('no arm matched');
  }
  let $t3;
  if(sn_2!==void 0&&$val(sn_2)!==void 0){
    $val(sn_2);
    $t3='some some';
  }else if(sn_2!==void 0&&$val(sn_2)===void 0){
    $t3='some none';
  }else if(sn_2===void 0){
    $t3='none';
  }else{
    $abort('no arm matched');
  }
  let $t5;
  if(nn_3!==void 0&&$val(nn_3)!==void 0){
    $val(nn_3);
    $t5='some some';
  }else if(nn_3!==void 0&&$val(nn_3)===void 0){
    $t5='some none';
  }else if(nn_3===void 0){
    $t5='none';
  }else{
    $abort('no arm matched');
  }
  const text_17=$t1+' | '+$t3+' | '+$t5;
  const self_18=$host_HostStdout_println(ctx_0[1],text_17);
  let $t7;
  if(self_18[0]===0){
    $t7=0;
  }else if(self_18[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  let $t9;
  if(ss_1!==void 0){
    const inner_22=$val(ss_1);
    $t9=inner_22;
  }else if(ss_1===void 0){
    $t9=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_23=$t9;
  let $t11;
  if(self_23!==void 0){
    $t11=self_23;
  }else if(self_23===void 0){
    $t11=-1n;
  }else{
    $abort('no arm matched');
  }
  let $t13;
  if(sn_2!==void 0){
    const inner_27=$val(sn_2);
    $t13=inner_27;
  }else if(sn_2===void 0){
    $t13=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_28=$t13;
  let $t15;
  if(self_28!==void 0){
    $t15=self_28;
  }else if(self_28===void 0){
    $t15=-1n;
  }else{
    $abort('no arm matched');
  }
  let $t17;
  if(nn_3!==void 0){
    const inner_32=$val(nn_3);
    $t17=inner_32;
  }else if(nn_3===void 0){
    $t17=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_33=$t17;
  let $t19;
  if(self_33!==void 0){
    $t19=self_33;
  }else if(self_33===void 0){
    $t19=-1n;
  }else{
    $abort('no arm matched');
  }
  const text_37=String($t11)+' '+String($t15)+' '+String($t19);
  const self_38=$host_HostStdout_println(ctx_0[1],text_37);
  let $t21;
  if(self_38[0]===0){
    $t21=0;
  }else if(self_38[0]===1){
    $t21=0;
  }else{
    $abort('no arm matched');
  }
  const o_90=$some($some(1n));
  let $t23;
  if(o_90===void 0){
    $t23=0n;
  }else if(o_90!==void 0&&$val(o_90)===void 0){
    $t23=1n;
  }else if(o_90!==void 0&&($val(o_90)!==void 0&&$val($val(o_90))===void 0)){
    $t23=2n;
  }else if(o_90!==void 0&&($val(o_90)!==void 0&&$val($val(o_90))!==void 0)){
    $t23=3n;
  }else{
    $abort('no arm matched');
  }
  const o_91=$some($some(void 0));
  let $t25;
  if(o_91===void 0){
    $t25=0n;
  }else if(o_91!==void 0&&$val(o_91)===void 0){
    $t25=1n;
  }else if(o_91!==void 0&&($val(o_91)!==void 0&&$val($val(o_91))===void 0)){
    $t25=2n;
  }else if(o_91!==void 0&&($val(o_91)!==void 0&&$val($val(o_91))!==void 0)){
    $t25=3n;
  }else{
    $abort('no arm matched');
  }
  const o_92=$some(void 0);
  let $t27;
  if(o_92===void 0){
    $t27=0n;
  }else if(o_92!==void 0&&$val(o_92)===void 0){
    $t27=1n;
  }else if(o_92!==void 0&&($val(o_92)!==void 0&&$val($val(o_92))===void 0)){
    $t27=2n;
  }else if(o_92!==void 0&&($val(o_92)!==void 0&&$val($val(o_92))!==void 0)){
    $t27=3n;
  }else{
    $abort('no arm matched');
  }
  let $t29;
  if(void 0===void 0){
    $t29=0n;
  }else if(void 0!==void 0&&$val(void 0)===void 0){
    $t29=1n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))===void 0)){
    $t29=2n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))!==void 0)){
    $t29=3n;
  }else{
    $abort('no arm matched');
  }
  const text_42=String($t23)+' '+String($t25)+' '+String($t27)+' '+String($t29);
  const self_43=$host_HostStdout_println(ctx_0[1],text_42);
  let $t31;
  if(self_43[0]===0){
    $t31=0;
  }else if(self_43[0]===1){
    $t31=0;
  }else{
    $abort('no arm matched');
  }
  const got_5=$list_get($k0,1n);
  let $t33;
  if(got_5!==void 0&&$val(got_5)!==void 0){
    $val(got_5);
    $t33='some some';
  }else if(got_5!==void 0&&$val(got_5)===void 0){
    $t33='some none';
  }else if(got_5===void 0){
    $t33='none';
  }else{
    $abort('no arm matched');
  }
  const text_49=$t33+' '+String($list_len($k0));
  const self_50=$host_HostStdout_println(ctx_0[1],text_49);
  let $t35;
  if(self_50[0]===0){
    $t35=0;
  }else if(self_50[0]===1){
    $t35=0;
  }else{
    $abort('no arm matched');
  }
  const text_54=$str($eqD0($k1,$k1))+' '+$str($eqD0($k1,$k2))+' '+$str($eqD0($k2,$k2));
  const self_55=$host_HostStdout_println(ctx_0[1],text_54);
  let $t37;
  if(self_55[0]===0){
    $t37=0;
  }else if(self_55[0]===1){
    $t37=0;
  }else{
    $abort('no arm matched');
  }
  const text_59=$show($k1,$D0)+' '+$show($k2,$D0);
  const self_60=$host_HostStdout_println(ctx_0[1],text_59);
  let $t39;
  if(self_60[0]===0){
    $t39=0;
  }else if(self_60[0]===1){
    $t39=0;
  }else{
    $abort('no arm matched');
  }
  let $t41;
  const $t42=2n;
  if($t42!==void 0){
    $t41=true;
  }else if($t42===void 0){
    $t41=false;
  }else{
    $abort('no arm matched');
  }
  let $t43;
  const $t44=void 0;
  if($t44!==void 0){
    $t43=false;
  }else if($t44===void 0){
    $t43=true;
  }else{
    $abort('no arm matched');
  }
  const text_68=$str($t41)+' '+$str($t43);
  const self_69=$host_HostStdout_println(ctx_0[1],text_68);
  let $t45;
  if(self_69[0]===0){
    $t45=0;
  }else if(self_69[0]===1){
    $t45=0;
  }else{
    $abort('no arm matched');
  }
  const sorted_8=$list_sortBy([$k1,$k2,$k3],ctx_0,(a_74,b_75)=>$cmp(a_74,b_75));
  const self_77=$list_get(sorted_8,0n);
  let $t47;
  if(self_77!==void 0){
    $t47=self_77;
  }else if(self_77===void 0){
    $t47=$k1;
  }else{
    $abort('no arm matched');
  }
  const lowest_9=$t47;
  const text_81=$show(lowest_9,$D0);
  const self_82=$host_HostStdout_println(ctx_0[1],text_81);
  let $t49;
  if(self_82[0]===0){
    $t49=0;
  }else if(self_82[0]===1){
    $t49=0;
  }else{
    $abort('no arm matched');
  }
  let $t51;
  const $t52=$cmp($k2,$k1);
  $t51=$t52===0;
  let $t53;
  const $t54=$cmp($k1,$k2);
  $t53=$t54===0;
  let $t55;
  const $t56=$cmp($k1,$k4);
  $t55=$t56===0;
  const text_86=$str($t51)+' '+$str($t53)+' '+$str($t55);
  const self_87=$host_HostStdout_println(ctx_0[1],text_86);
  let $t57;
  if(self_87[0]===0){
    $t57=0;
  }else if(self_87[0]===1){
    $t57=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}

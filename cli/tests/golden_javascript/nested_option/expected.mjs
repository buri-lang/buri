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
  let $t11;
  if(ss_1!==void 0){
    const inner_22=$val(ss_1);
    $t11=inner_22;
  }else if(ss_1===void 0){
    $t11=void 0;
  }else{
    $abort('no arm matched');
  }
  let $t14;
  if(sn_2!==void 0){
    const inner_24=$val(sn_2);
    $t14=inner_24;
  }else if(sn_2===void 0){
    $t14=void 0;
  }else{
    $abort('no arm matched');
  }
  let $t17;
  if(nn_3!==void 0){
    const inner_26=$val(nn_3);
    $t17=inner_26;
  }else if(nn_3===void 0){
    $t17=void 0;
  }else{
    $abort('no arm matched');
  }
  const text_28=String($t11!==void 0?$t11:-1n)+' '+String($t14!==void 0?$t14:-1n)+' '+String($t17!==void 0?$t17:-1n);
  const self_29=$host_HostStdout_println(ctx_0[1],text_28);
  let $t18;
  if(self_29[0]===0){
    $t18=0;
  }else if(self_29[0]===1){
    $t18=0;
  }else{
    $abort('no arm matched');
  }
  const o_78=$some($some(1n));
  let $t20;
  if(o_78===void 0){
    $t20=0n;
  }else if(o_78!==void 0&&$val(o_78)===void 0){
    $t20=1n;
  }else if(o_78!==void 0&&($val(o_78)!==void 0&&$val($val(o_78))===void 0)){
    $t20=2n;
  }else if(o_78!==void 0&&($val(o_78)!==void 0&&$val($val(o_78))!==void 0)){
    $t20=3n;
  }else{
    $abort('no arm matched');
  }
  const o_79=$some($some(void 0));
  let $t22;
  if(o_79===void 0){
    $t22=0n;
  }else if(o_79!==void 0&&$val(o_79)===void 0){
    $t22=1n;
  }else if(o_79!==void 0&&($val(o_79)!==void 0&&$val($val(o_79))===void 0)){
    $t22=2n;
  }else if(o_79!==void 0&&($val(o_79)!==void 0&&$val($val(o_79))!==void 0)){
    $t22=3n;
  }else{
    $abort('no arm matched');
  }
  const o_80=$some(void 0);
  let $t24;
  if(o_80===void 0){
    $t24=0n;
  }else if(o_80!==void 0&&$val(o_80)===void 0){
    $t24=1n;
  }else if(o_80!==void 0&&($val(o_80)!==void 0&&$val($val(o_80))===void 0)){
    $t24=2n;
  }else if(o_80!==void 0&&($val(o_80)!==void 0&&$val($val(o_80))!==void 0)){
    $t24=3n;
  }else{
    $abort('no arm matched');
  }
  let $t26;
  if(void 0===void 0){
    $t26=0n;
  }else if(void 0!==void 0&&$val(void 0)===void 0){
    $t26=1n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))===void 0)){
    $t26=2n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))!==void 0)){
    $t26=3n;
  }else{
    $abort('no arm matched');
  }
  const text_33=String($t20)+' '+String($t22)+' '+String($t24)+' '+String($t26);
  const self_34=$host_HostStdout_println(ctx_0[1],text_33);
  let $t28;
  if(self_34[0]===0){
    $t28=0;
  }else if(self_34[0]===1){
    $t28=0;
  }else{
    $abort('no arm matched');
  }
  const got_5=$list_get($k0,1n);
  let $t30;
  if(got_5!==void 0&&$val(got_5)!==void 0){
    $val(got_5);
    $t30='some some';
  }else if(got_5!==void 0&&$val(got_5)===void 0){
    $t30='some none';
  }else if(got_5===void 0){
    $t30='none';
  }else{
    $abort('no arm matched');
  }
  const text_40=$t30+' '+String($list_len($k0));
  const self_41=$host_HostStdout_println(ctx_0[1],text_40);
  let $t32;
  if(self_41[0]===0){
    $t32=0;
  }else if(self_41[0]===1){
    $t32=0;
  }else{
    $abort('no arm matched');
  }
  const text_45=$str($eqD0($k1,$k1))+' '+$str($eqD0($k1,$k2))+' '+$str($eqD0($k2,$k2));
  const self_46=$host_HostStdout_println(ctx_0[1],text_45);
  let $t34;
  if(self_46[0]===0){
    $t34=0;
  }else if(self_46[0]===1){
    $t34=0;
  }else{
    $abort('no arm matched');
  }
  const text_50=$show($k1,$D0)+' '+$show($k2,$D0);
  const self_51=$host_HostStdout_println(ctx_0[1],text_50);
  let $t36;
  if(self_51[0]===0){
    $t36=0;
  }else if(self_51[0]===1){
    $t36=0;
  }else{
    $abort('no arm matched');
  }
  let $t38;
  const $t39=$k1[0];
  if($t39!==void 0){
    $t38=true;
  }else if($t39===void 0){
    $t38=false;
  }else{
    $abort('no arm matched');
  }
  let $t40;
  const $t41=$k2[0];
  if($t41!==void 0){
    $t40=false;
  }else if($t41===void 0){
    $t40=true;
  }else{
    $abort('no arm matched');
  }
  const text_59=$str($t38)+' '+$str($t40);
  const self_60=$host_HostStdout_println(ctx_0[1],text_59);
  let $t42;
  if(self_60[0]===0){
    $t42=0;
  }else if(self_60[0]===1){
    $t42=0;
  }else{
    $abort('no arm matched');
  }
  const sorted_8=$list_sortBy([$k1,$k2,$k3],ctx_0,(a_65,b_66)=>$cmp(a_65,b_66));
  const $t44=$list_get(sorted_8,0n);
  const text_69=$show($t44!==void 0?$t44:$k1,$D0);
  const self_70=$host_HostStdout_println(ctx_0[1],text_69);
  let $t45;
  if(self_70[0]===0){
    $t45=0;
  }else if(self_70[0]===1){
    $t45=0;
  }else{
    $abort('no arm matched');
  }
  let $t47;
  const $t48=$cmp($k2,$k1);
  $t47=$t48===0;
  let $t49;
  const $t50=$cmp($k1,$k2);
  $t49=$t50===0;
  let $t51;
  const $t52=$cmp($k1,$k4);
  $t51=$t52===0;
  const text_74=$str($t47)+' '+$str($t49)+' '+$str($t51);
  const self_75=$host_HostStdout_println(ctx_0[1],text_74);
  let $t53;
  if(self_75[0]===0){
    $t53=0;
  }else if(self_75[0]===1){
    $t53=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}

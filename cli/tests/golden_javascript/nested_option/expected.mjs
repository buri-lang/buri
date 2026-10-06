const $k0=[1n,void 0,3n];
const $k1=[2n];
const $k2=[void 0];
const $k3=[1n];
const $k4=[9n];
const $k5=[0,0];
const $k6=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
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
function __cmd_x_main_buri$main$withHost(host_0){
  const ctx_1=[host_0[0],host_0[1]];
  const ss_2=$some(7n);
  const sn_3=$some(void 0);
  const nn_4=void 0;
  let $t1;
  if(ss_2!==void 0&&$val(ss_2)!==void 0){
    $val(ss_2);
    $t1='some some';
  }else if(ss_2!==void 0&&$val(ss_2)===void 0){
    $t1='some none';
  }else if(ss_2===void 0){
    $t1='none';
  }else{
    $abort('no arm matched');
  }
  let $t3;
  if(sn_3!==void 0&&$val(sn_3)!==void 0){
    $val(sn_3);
    $t3='some some';
  }else if(sn_3!==void 0&&$val(sn_3)===void 0){
    $t3='some none';
  }else if(sn_3===void 0){
    $t3='none';
  }else{
    $abort('no arm matched');
  }
  let $t5;
  if(nn_4!==void 0&&$val(nn_4)!==void 0){
    $val(nn_4);
    $t5='some some';
  }else if(nn_4!==void 0&&$val(nn_4)===void 0){
    $t5='some none';
  }else if(nn_4===void 0){
    $t5='none';
  }else{
    $abort('no arm matched');
  }
  const text_18=$t1+' | '+$t3+' | '+$t5;
  const self_19=$host_HostStdout_println(ctx_1[1],text_18);
  let $t7;
  if(self_19[0]===0){
    $t7=0;
  }else if(self_19[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  let $t9;
  if(ss_2!==void 0){
    const inner_23=$val(ss_2);
    $t9=inner_23;
  }else if(ss_2===void 0){
    $t9=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_24=$t9;
  let $t11;
  if(self_24!==void 0){
    $t11=self_24;
  }else if(self_24===void 0){
    $t11=-1n;
  }else{
    $abort('no arm matched');
  }
  let $t13;
  if(sn_3!==void 0){
    const inner_28=$val(sn_3);
    $t13=inner_28;
  }else if(sn_3===void 0){
    $t13=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_29=$t13;
  let $t15;
  if(self_29!==void 0){
    $t15=self_29;
  }else if(self_29===void 0){
    $t15=-1n;
  }else{
    $abort('no arm matched');
  }
  let $t17;
  if(nn_4!==void 0){
    const inner_33=$val(nn_4);
    $t17=inner_33;
  }else if(nn_4===void 0){
    $t17=void 0;
  }else{
    $abort('no arm matched');
  }
  const self_34=$t17;
  let $t19;
  if(self_34!==void 0){
    $t19=self_34;
  }else if(self_34===void 0){
    $t19=-1n;
  }else{
    $abort('no arm matched');
  }
  const text_38=String($t11)+' '+String($t15)+' '+String($t19);
  const self_39=$host_HostStdout_println(ctx_1[1],text_38);
  let $t21;
  if(self_39[0]===0){
    $t21=0;
  }else if(self_39[0]===1){
    $t21=0;
  }else{
    $abort('no arm matched');
  }
  const o_91=$some($some(1n));
  let $t23;
  if(o_91===void 0){
    $t23=0n;
  }else if(o_91!==void 0&&$val(o_91)===void 0){
    $t23=1n;
  }else if(o_91!==void 0&&($val(o_91)!==void 0&&$val($val(o_91))===void 0)){
    $t23=2n;
  }else if(o_91!==void 0&&($val(o_91)!==void 0&&$val($val(o_91))!==void 0)){
    $t23=3n;
  }else{
    $abort('no arm matched');
  }
  const o_92=$some($some(void 0));
  let $t25;
  if(o_92===void 0){
    $t25=0n;
  }else if(o_92!==void 0&&$val(o_92)===void 0){
    $t25=1n;
  }else if(o_92!==void 0&&($val(o_92)!==void 0&&$val($val(o_92))===void 0)){
    $t25=2n;
  }else if(o_92!==void 0&&($val(o_92)!==void 0&&$val($val(o_92))!==void 0)){
    $t25=3n;
  }else{
    $abort('no arm matched');
  }
  const o_93=$some(void 0);
  let $t27;
  if(o_93===void 0){
    $t27=0n;
  }else if(o_93!==void 0&&$val(o_93)===void 0){
    $t27=1n;
  }else if(o_93!==void 0&&($val(o_93)!==void 0&&$val($val(o_93))===void 0)){
    $t27=2n;
  }else if(o_93!==void 0&&($val(o_93)!==void 0&&$val($val(o_93))!==void 0)){
    $t27=3n;
  }else{
    $abort('no arm matched');
  }
  const text_43=String($t23)+' '+String($t25)+' '+String($t27)+' '+String(0n);
  const self_44=$host_HostStdout_println(ctx_1[1],text_43);
  let $t31;
  if(self_44[0]===0){
    $t31=0;
  }else if(self_44[0]===1){
    $t31=0;
  }else{
    $abort('no arm matched');
  }
  const got_6=$list_get($k0,1n);
  let $t33;
  if(got_6!==void 0&&$val(got_6)!==void 0){
    $val(got_6);
    $t33='some some';
  }else if(got_6!==void 0&&$val(got_6)===void 0){
    $t33='some none';
  }else if(got_6===void 0){
    $t33='none';
  }else{
    $abort('no arm matched');
  }
  const text_50=$t33+' '+String($list_length($k0));
  const self_51=$host_HostStdout_println(ctx_1[1],text_50);
  let $t35;
  if(self_51[0]===0){
    $t35=0;
  }else if(self_51[0]===1){
    $t35=0;
  }else{
    $abort('no arm matched');
  }
  const text_55=$str($eqD0($k1,$k1))+' '+$str($eqD0($k1,$k2))+' '+$str($eqD0($k2,$k2));
  const self_56=$host_HostStdout_println(ctx_1[1],text_55);
  let $t37;
  if(self_56[0]===0){
    $t37=0;
  }else if(self_56[0]===1){
    $t37=0;
  }else{
    $abort('no arm matched');
  }
  const text_60=$show($k1,$D0)+' '+$show($k2,$D0);
  const self_61=$host_HostStdout_println(ctx_1[1],text_60);
  let $t39;
  if(self_61[0]===0){
    $t39=0;
  }else if(self_61[0]===1){
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
  const text_69=$str($t41)+' '+$str($t43);
  const self_70=$host_HostStdout_println(ctx_1[1],text_69);
  let $t45;
  if(self_70[0]===0){
    $t45=0;
  }else if(self_70[0]===1){
    $t45=0;
  }else{
    $abort('no arm matched');
  }
  const sorted_9=$list_sortBy([$k1,$k2,$k3],ctx_1,(a_75,b_76)=>$cmp(a_75,b_76));
  const self_78=$list_getFlat(sorted_9,0n);
  let $t47;
  if(self_78!==void 0){
    $t47=self_78;
  }else if(self_78===void 0){
    $t47=$k1;
  }else{
    $abort('no arm matched');
  }
  const lowest_10=$t47;
  const text_82=$show(lowest_10,$D0);
  const self_83=$host_HostStdout_println(ctx_1[1],text_82);
  let $t49;
  if(self_83[0]===0){
    $t49=0;
  }else if(self_83[0]===1){
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
  const text_87=$str($t51)+' '+$str($t53)+' '+$str($t55);
  const self_88=$host_HostStdout_println(ctx_1[1],text_87);
  let $t57;
  if(self_88[0]===0){
    $t57=0;
  }else if(self_88[0]===1){
    $t57=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}
function __cmd_x_main_buri$main(){
  return __cmd_x_main_buri$main$withHost($k6);
}

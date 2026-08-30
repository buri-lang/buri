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
  $host_HostStdout_println(ctx_0[1],$t1+' | '+$t3+' | '+$t5);
  let $t10;
  if(ss_1!==void 0){
    const inner_17=$val(ss_1);
    $t10=inner_17;
  }else if(ss_1===void 0){
    $t10=void 0;
  }else{
    $abort('no arm matched');
  }
  let $t13;
  if(sn_2!==void 0){
    const inner_19=$val(sn_2);
    $t13=inner_19;
  }else if(sn_2===void 0){
    $t13=void 0;
  }else{
    $abort('no arm matched');
  }
  let $t16;
  if(nn_3!==void 0){
    const inner_21=$val(nn_3);
    $t16=inner_21;
  }else if(nn_3===void 0){
    $t16=void 0;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],String($t10!==void 0?$t10:-1n)+' '+String($t13!==void 0?$t13:-1n)+' '+String($t16!==void 0?$t16:-1n));
  const o_33=$some($some(1n));
  let $t18;
  if(o_33===void 0){
    $t18=0n;
  }else if(o_33!==void 0&&$val(o_33)===void 0){
    $t18=1n;
  }else if(o_33!==void 0&&($val(o_33)!==void 0&&$val($val(o_33))===void 0)){
    $t18=2n;
  }else if(o_33!==void 0&&($val(o_33)!==void 0&&$val($val(o_33))!==void 0)){
    $t18=3n;
  }else{
    $abort('no arm matched');
  }
  const o_34=$some($some(void 0));
  let $t20;
  if(o_34===void 0){
    $t20=0n;
  }else if(o_34!==void 0&&$val(o_34)===void 0){
    $t20=1n;
  }else if(o_34!==void 0&&($val(o_34)!==void 0&&$val($val(o_34))===void 0)){
    $t20=2n;
  }else if(o_34!==void 0&&($val(o_34)!==void 0&&$val($val(o_34))!==void 0)){
    $t20=3n;
  }else{
    $abort('no arm matched');
  }
  const o_35=$some(void 0);
  let $t22;
  if(o_35===void 0){
    $t22=0n;
  }else if(o_35!==void 0&&$val(o_35)===void 0){
    $t22=1n;
  }else if(o_35!==void 0&&($val(o_35)!==void 0&&$val($val(o_35))===void 0)){
    $t22=2n;
  }else if(o_35!==void 0&&($val(o_35)!==void 0&&$val($val(o_35))!==void 0)){
    $t22=3n;
  }else{
    $abort('no arm matched');
  }
  let $t24;
  if(void 0===void 0){
    $t24=0n;
  }else if(void 0!==void 0&&$val(void 0)===void 0){
    $t24=1n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))===void 0)){
    $t24=2n;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))!==void 0)){
    $t24=3n;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],String($t18)+' '+String($t20)+' '+String($t22)+' '+String($t24));
  const got_5=$list_get($k0,1n);
  let $t27;
  if(got_5!==void 0&&$val(got_5)!==void 0){
    $val(got_5);
    $t27='some some';
  }else if(got_5!==void 0&&$val(got_5)===void 0){
    $t27='some none';
  }else if(got_5===void 0){
    $t27='none';
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],$t27+' '+String($list_len($k0)));
  $host_HostStdout_println(ctx_0[1],$str($eqD0($k1,$k1))+' '+$str($eqD0($k1,$k2))+' '+$str($eqD0($k2,$k2)));
  $host_HostStdout_println(ctx_0[1],$show($k1,$D0)+' '+$show($k2,$D0));
  let $t30;
  const $t31=$k1[0];
  if($t31!==void 0){
    $t30=true;
  }else if($t31===void 0){
    $t30=false;
  }else{
    $abort('no arm matched');
  }
  let $t32;
  const $t33=$k2[0];
  if($t33!==void 0){
    $t32=false;
  }else if($t33===void 0){
    $t32=true;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],$str($t30)+' '+$str($t32));
  const sorted_8=$list_sortBy([$k1,$k2,$k3],ctx_0,(a_30,b_31)=>$cmp(a_30,b_31));
  const $t35=$list_get(sorted_8,0n);
  $host_HostStdout_println(ctx_0[1],$show($t35!==void 0?$t35:$k1,$D0));
  let $t36;
  const $t37=$cmp($k2,$k1);
  $t36=$t37===0;
  let $t38;
  const $t39=$cmp($k1,$k2);
  $t38=$t39===0;
  let $t40;
  const $t41=$cmp($k1,$k4);
  $t40=$t41===0;
  $host_HostStdout_println(ctx_0[1],$str($t36)+' '+$str($t38)+' '+$str($t40));
  return $k5;
}

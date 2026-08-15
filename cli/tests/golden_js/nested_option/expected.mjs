const $k0=[1,void 0,3];
const $k1=[2];
const $k2=[void 0];
const $k3=[1];
const $k4=[9];
const $k5=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
$D0.push(2,'Holder',true,['inner'],[$D1]);
$D1.push(7,$D2);
$D2.push(0,'i');
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const ss_1=$some(7);
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
  let $t8;
  if(ss_1!==void 0){
    const inner_17=$val(ss_1);
    $t8=inner_17;
  }else if(ss_1===void 0){
    $t8=void 0;
  }else{
    $abort('no arm matched');
  }
  const $t10=$t8;
  let $t11;
  if(sn_2!==void 0){
    const inner_19=$val(sn_2);
    $t11=inner_19;
  }else if(sn_2===void 0){
    $t11=void 0;
  }else{
    $abort('no arm matched');
  }
  const $t13=$t11;
  let $t14;
  if(nn_3!==void 0){
    const inner_21=$val(nn_3);
    $t14=inner_21;
  }else if(nn_3===void 0){
    $t14=void 0;
  }else{
    $abort('no arm matched');
  }
  const $t16=$t14;
  $host_HostStdout_println(ctx_0[1],String($t10!==void 0?$t10:-1)+' '+String($t13!==void 0?$t13:-1)+' '+String($t16!==void 0?$t16:-1));
  const o_33=$some($some(1));
  let $t18;
  if(o_33===void 0){
    $t18=0;
  }else if(o_33!==void 0&&$val(o_33)===void 0){
    $t18=1;
  }else if(o_33!==void 0&&($val(o_33)!==void 0&&$val($val(o_33))===void 0)){
    $t18=2;
  }else if(o_33!==void 0&&($val(o_33)!==void 0&&$val($val(o_33))!==void 0)){
    $t18=3;
  }else{
    $abort('no arm matched');
  }
  const o_34=$some($some(void 0));
  let $t20;
  if(o_34===void 0){
    $t20=0;
  }else if(o_34!==void 0&&$val(o_34)===void 0){
    $t20=1;
  }else if(o_34!==void 0&&($val(o_34)!==void 0&&$val($val(o_34))===void 0)){
    $t20=2;
  }else if(o_34!==void 0&&($val(o_34)!==void 0&&$val($val(o_34))!==void 0)){
    $t20=3;
  }else{
    $abort('no arm matched');
  }
  const o_35=$some(void 0);
  let $t22;
  if(o_35===void 0){
    $t22=0;
  }else if(o_35!==void 0&&$val(o_35)===void 0){
    $t22=1;
  }else if(o_35!==void 0&&($val(o_35)!==void 0&&$val($val(o_35))===void 0)){
    $t22=2;
  }else if(o_35!==void 0&&($val(o_35)!==void 0&&$val($val(o_35))!==void 0)){
    $t22=3;
  }else{
    $abort('no arm matched');
  }
  let $t24;
  if(void 0===void 0){
    $t24=0;
  }else if(void 0!==void 0&&$val(void 0)===void 0){
    $t24=1;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))===void 0)){
    $t24=2;
  }else if(void 0!==void 0&&($val(void 0)!==void 0&&$val($val(void 0))!==void 0)){
    $t24=3;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],String($t18)+' '+String($t20)+' '+String($t22)+' '+String($t24));
  const got_5=$list_get($k0,1);
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
  $host_HostStdout_println(ctx_0[1],$str($eq($k1,$k1))+' '+$str($eq($k1,$k2))+' '+$str($eq($k2,$k2)));
  $host_HostStdout_println(ctx_0[1],$show($k1,$D0)+' '+$show($k2,$D0));
  let $t30;
  const $t31=2;
  if($t31!==void 0){
    $t30=true;
  }else if($t31===void 0){
    $t30=false;
  }else{
    $abort('no arm matched');
  }
  let $t32;
  const $t33=void 0;
  if($t33!==void 0){
    $t32=false;
  }else if($t33===void 0){
    $t32=true;
  }else{
    $abort('no arm matched');
  }
  $host_HostStdout_println(ctx_0[1],$str($t30)+' '+$str($t32));
  const sorted_8=$list_sortBy([$k1,$k2,$k3],ctx_0,(a_30,b_31)=>$cmp(a_30,b_31));
  const $t35=$list_get(sorted_8,0);
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

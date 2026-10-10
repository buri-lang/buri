const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n,'x'];
const $k2=[1n,'y'];
const $k3=[0];
const $k4=[1,2n];
const $k5=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
$D0.push(2,'Pair',true,['a','b'],[$D1,$D2]);
$D1.push(0,'I','I64');
$D2.push(0,'s');
$D3.push(3,'Tag',[['Low',false,[],[]],['High',false,['0'],[$D1]]],false);
function $eqD0(a,b){
  if(a===b){
    return true;
  }
  return a[0]===b[0]&&a[1]===b[1];
}
function $eqD3(a,b){
  if(a===b){
    return true;
  }
  if(a[0]!==b[0]){
    return false;
  }
  switch(a[0]){
    case 0:
      return true;
    case 1:
      return a[1]===b[1];
    default:
      return false;
  }
}
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_5=$str($eqD0($k1,$k2))+' '+$str($eqD0($k1,$k1));
  const self_6=$host_HostStdout_println(ctx_1[1],text_5);
  let $t1;
  if(self_6[0]===0){
    $t1=0;
  }else if(self_6[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_10=$show($k1,$D0)+' '+$show($k2,$D0);
  const self_11=$host_HostStdout_println(ctx_1[1],text_10);
  let $t3;
  if(self_11[0]===0){
    $t3=0;
  }else if(self_11[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_15=$str($eqD3($k3,$k4))+' '+$show($k4,$D3);
  const self_16=$host_HostStdout_println(ctx_1[1],text_15);
  let $t5;
  if(self_16[0]===0){
    $t5=0;
  }else if(self_16[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k5;
}

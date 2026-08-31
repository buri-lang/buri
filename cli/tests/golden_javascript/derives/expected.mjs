const $k0=[1n,'x'];
const $k1=[1n,'y'];
const $k2=[0];
const $k3=[1,2n];
const $k4=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
$D0.push(2,'Pair',true,['a','b'],[$D1,$D2]);
$D1.push(0,'I');
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
  const ctx_0=[[],[]];
  const text_4=$str($eqD0($k0,$k1))+' '+$str($eqD0($k0,$k0));
  const self_5=$host_HostStdout_println(ctx_0[1],text_4);
  let $t1;
  if(self_5[0]===0){
    $t1=0;
  }else if(self_5[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_9=$show($k0,$D0)+' '+$show($k1,$D0);
  const self_10=$host_HostStdout_println(ctx_0[1],text_9);
  let $t3;
  if(self_10[0]===0){
    $t3=0;
  }else if(self_10[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_14=$str($eqD3($k2,$k3))+' '+$show($k3,$D3);
  const self_15=$host_HostStdout_println(ctx_0[1],text_14);
  let $t5;
  if(self_15[0]===0){
    $t5=0;
  }else if(self_15[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
}

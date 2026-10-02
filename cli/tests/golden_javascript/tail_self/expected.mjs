const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_3=String(__cmd_x_main_buri$sumTo(100n,0n))+' '+String(__cmd_x_main_buri$fib(30n,0n,1n))+' '+String(__cmd_x_main_buri$countDigits(12345n,0n));
  const self_4=$host_HostStdout_println(ctx_1[1],text_3);
  let $t1;
  if(self_4[0]===0){
    $t1=0;
  }else if(self_4[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_8=String(__cmd_x_main_buri$swapDown(1n,2n,3n))+' '+String(__cmd_x_main_buri$swapDown(1n,2n,4n));
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t3;
  if(self_9[0]===0){
    $t3=0;
  }else if(self_9[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k1;
}
function __cmd_x_main_buri$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0n){
      return acc_1;
    }else{
      const $t1=n_0-1n;
      acc_1=acc_1+n_0;
      n_0=$t1;
      continue;
    }
  }
}
function __cmd_x_main_buri$fib(n_0,a_1,b_2){
  while(true){
    if(n_0===0n){
      return a_1;
    }else{
      n_0=n_0-1n;
      const $t1=b_2;
      b_2=a_1+b_2;
      a_1=$t1;
      continue;
    }
  }
}
function __cmd_x_main_buri$countDigits(n_0,acc_1){
  while(true){
    if(n_0<10n){
      return acc_1+1n;
    }else{
      n_0=n_0/10n;
      acc_1=acc_1+1n;
      continue;
    }
  }
}
function __cmd_x_main_buri$swapDown(a_0,b_1,fuel_2){
  while(true){
    if(fuel_2===0n){
      return a_0*10n+b_1;
    }else{
      const $t1=b_1;
      b_1=a_0;
      fuel_2=fuel_2-1n;
      a_0=$t1;
      continue;
    }
  }
}

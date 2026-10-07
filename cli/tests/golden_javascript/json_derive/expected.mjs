const $k0=[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]];
const $k1=[1n,2n];
const $k2=[1,3n,4n];
const $k3=[0];
const $k4=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
const $D4=[];
const $D5=[];
$D0.push(3,'Result',[['Ok',false,['0'],[$D1]],['Err',false,['0'],[$D3]]],false);
$D1.push(2,'Point',true,['x','y'],[$D2,$D2]);
$D2.push(0,'I');
$D3.push(3,'DecodeError',[['Missing',true,['path'],[$D4]],['WrongType',true,['path','wanted','found'],[$D4,$D4,$D4]],['UnknownVariant',true,['path','tag'],[$D4,$D4]]],false);
$D4.push(0,'s');
$D5.push(3,'Shape',[['Empty',false,[],[]],['Rect',true,['width','height'],[$D2,$D2]]],false);
function $eqD0(a,b){
  if(a===b){
    return true;
  }
  if(a[0]!==b[0]){
    return false;
  }
  switch(a[0]){
    case 0:
      return $eqD1(a[1],b[1]);
    case 1:
      return $eqD3(a[1],b[1]);
    default:
      return false;
  }
}
function $eqD1(a,b){
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
      return a[1]===b[1];
    case 1:
      return a[1]===b[1]&&a[2]===b[2]&&a[3]===b[3];
    case 2:
      return a[1]===b[1]&&a[2]===b[2];
    default:
      return false;
  }
}
function __cmd_x_main_buri$main(){
  const ctx_1=[$k0[0],$k0[1]];
  const text_8=core_json$stringify$3qb9ap(ctx_1,$json_of($k1,$D1));
  const self_9=$host_HostStdout_println(ctx_1[1],text_8);
  let $t1;
  if(self_9[0]===0){
    $t1=0;
  }else if(self_9[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_15=core_json$stringify$3qb9ap(ctx_1,$json_of($k2,$D5));
  const self_16=$host_HostStdout_println(ctx_1[1],text_15);
  let $t3;
  if(self_16[0]===0){
    $t3=0;
  }else if(self_16[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  const text_22=core_json$stringify$3qb9ap(ctx_1,$json_of($k3,$D5));
  const self_23=$host_HostStdout_println(ctx_1[1],text_22);
  let $t5;
  if(self_23[0]===0){
    $t5=0;
  }else if(self_23[0]===1){
    $t5=0;
  }else{
    $abort('no arm matched');
  }
  const back_4=$json_decode($json_of($k1,$D1),$D1);
  const text_29=$str($eqD0(back_4,[0,$k1]));
  const self_30=$host_HostStdout_println(ctx_1[1],text_29);
  let $t7;
  if(self_30[0]===0){
    $t7=0;
  }else if(self_30[0]===1){
    $t7=0;
  }else{
    $abort('no arm matched');
  }
  return $k4;
}
function core_json$stringify$3qb9ap(ctx_0,value_1){
  switch(value_1[0]){
    case 0:
      {
        return 'null';
      }
    case 1:
      {
        return value_1[1]?'true':'false';
      }
    case 2:
      {
        const s_17=$str_fromFloat(ctx_0,value_1[1]);
        return $str_endsWith(s_17,'.0')?$str_slice(s_17,0n,$str_length(s_17)-2n):s_17;
      }
    case 3:
      {
        return core_json$quote$3qb9ap(ctx_0,value_1[1]);
      }
    case 4:
      {
        const items_5=value_1[1];
        $share(items_5);
        const parts_8=$list_mapCtx(items_5,ctx_0,(c_6,item_7)=>core_json$stringify$3qb9ap(c_6,item_7));
        return $str_format(ctx_0,'['+$list_join(parts_8,ctx_0,',')+']');
      }
    case 5:
      {
        const entries_9=value_1[1];
        $share(entries_9);
        const parts_14=$list_mapCtx(entries_9,ctx_0,(c_10,e_11)=>{
          const item_13=e_11[1];
          $fromShared(e_11,item_13);
          return $str_format(c_10,core_json$quote$3qb9ap(c_10,e_11[0])+':'+core_json$stringify$3qb9ap(c_10,item_13));
        });
        return $str_format(ctx_0,'{'+$list_join(parts_14,ctx_0,',')+'}');
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function core_json$quote$3qb9ap(ctx_0,text_1){
  const inner_4=$list_join($list_mapCtx($str_chars(text_1,ctx_0),ctx_0,(c_2,ch_3)=>{
    if(ch_3==='"'){
      return '\\"';
    }else if(ch_3==='\\'){
      return '\\\\';
    }else if(ch_3==='\n'){
      return '\\n';
    }else if(ch_3==='\r'){
      return '\\r';
    }else if(ch_3==='\t'){
      return '\\t';
    }else if(BigInt($character_toU32(ch_3))<32n){
      const n_7=BigInt($character_toU32(ch_3));
      const self_8=$str_charAt('0123456789abcdef',n_7/16n);
      let $t1;
      if(self_8!==void 0){
        $t1=self_8;
      }else if(self_8===void 0){
        $t1='0';
      }else{
        $abort('no arm matched');
      }
      const self_11=$str_charAt('0123456789abcdef',n_7%16n);
      let $t3;
      if(self_11!==void 0){
        $t3=self_11;
      }else if(self_11===void 0){
        $t3='0';
      }else{
        $abort('no arm matched');
      }
      return $str_format(c_2,'\\u00'+$t1+$t3);
    }else{
      return $str_fromChars(c_2,[ch_3]);
    }
  }),ctx_0,'');
  return $str_format(ctx_0,'"'+inner_4+'"');
}
